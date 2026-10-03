//! Getting bytes between the host and the tablet over USB.
//!
//! Three `adb forward`s are set up rather than one. It costs almost nothing and
//! it removes head-of-line blocking: a touch event should never wait behind a
//! 30 KB video frame that is already half-written to the socket.
//!
//! Direction of travel is always the same -- the host *connects*, the tablet
//! *listens* on abstract unix sockets. That means the companion app can be
//! started first and simply wait, and it avoids needing `adb reverse`, which is
//! less reliable across reconnects.

use std::path::Path;
use std::time::Duration;

use tokio::net::TcpStream;
use tracing::{debug, info, warn};
use xs_proto::ports;

pub mod adb;
mod aoa;

pub use xs_proto::TransportMode;

pub trait Stream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin> Stream for T {}
pub type TransportStream = Box<dyn Stream>;
pub mod frame;

pub use adb::{Adb, Device, DeviceState};
pub use frame::{Frame, FrameReader, FrameWriter};

/// Android package of the companion app.
pub const PACKAGE: &str = "io.github.tymonoman.extraspace";
/// Activity that renders the mirrored display.
pub const ACTIVITY: &str = "io.github.tymonoman.extraspace/.MirrorActivity";

/// Abstract socket names the companion app listens on. Must match `Sockets.kt`.
pub mod sockets {
    pub const CONTROL: &str = "extraspace-control";
    pub const VIDEO: &str = "extraspace-video";
    pub const CAMERA: &str = "extraspace-camera";
}

/// How long to keep retrying the initial connect while the app starts up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(150);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Incompatible(String),

    #[error("The shared connection method is USB accessory")]
    SwitchToAccessory,

    #[error("USB accessory: {0}")]
    Accessory(String),

    #[error(transparent)]
    Adb(#[from] adb::Error),

    #[error(transparent)]
    Frame(#[from] frame::Error),

    #[error("{0}")]
    Companion(String),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error(
        "the companion app did not accept a connection on port {port} within {}s. \
         It may have crashed on startup -- check `adb logcat -s extraspace`.",
        CONNECT_TIMEOUT.as_secs()
    )]
    ConnectTimeout { port: u16 },
}

pub type Result<T> = std::result::Result<T, Error>;

/// A connected tablet with all three channels established.
pub struct Transport {
    pub device: Device,
    pub hello: Option<xs_proto::Hello>,
    pub control: TransportStream,
    pub video: TransportStream,
    pub camera: TransportStream,
    adb: Adb,
    simulated: bool,
    accessory: Option<std::sync::Arc<aoa::AccessoryHandle>>,
}

/// The teardown half of a [`Transport`], kept after the sockets are handed out.
///
/// Splitting these apart lets the caller move each socket into its own task
/// while still holding something that can undo the adb forwards afterwards.
#[derive(Clone)]
pub struct TransportHandle {
    pub device: Device,
    adb: Adb,
    /// True for the fake-tablet path, where there is nothing for adb to undo.
    simulated: bool,
    accessory: Option<std::sync::Arc<aoa::AccessoryHandle>>,
}

/// Undo forwards/USB ownership if setup is cancelled while awaiting user consent.
pub struct PendingConnection(Option<TransportHandle>);
impl PendingConnection {
    pub fn new(handle: TransportHandle) -> Self {
        Self(Some(handle))
    }
    pub fn disarm(&mut self) {
        self.0 = None;
    }
}
impl Drop for PendingConnection {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    handle.disconnect().await;
                });
            }
        }
    }
}

impl TransportHandle {
    /// Removes forwards or closes accessory streaming; Android returns to setup.
    pub async fn disconnect(&self) {
        if let Some(accessory) = &self.accessory {
            accessory.disconnect().await;
            return;
        }
        if self.simulated {
            debug!("simulated transport: nothing to tear down");
            return;
        }
        for port in [ports::CONTROL, ports::VIDEO, ports::CAMERA] {
            self.adb.remove_forward(&self.device.serial, port).await;
        }
        debug!("transport torn down; companion remains available for setup");
    }
}

/// Environment variable that swaps the real tablet for anything listening on the
/// three ports locally.
///
/// This exists so the host can be developed and tested without hardware --
/// see `cargo run -p xs-core --example fake_tablet`. It is checked at connect
/// time rather than behind a cargo feature so a release build can still use it.
pub const FAKE_TABLET_ENV: &str = "EXTRASPACE_FAKE_TABLET";

fn fake_tablet_requested() -> bool {
    std::env::var_os(FAKE_TABLET_ENV).is_some_and(|v| v != "0" && !v.is_empty())
}

pub async fn read_hello<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
) -> Result<xs_proto::Hello> {
    for _ in 0..128 {
        let frame = reader.read_frame().await?;
        if frame.header.channel != xs_proto::Channel::Control {
            continue;
        }
        if frame.header.kind == xs_proto::ControlKind::Error as u8 {
            return Err(Error::Companion(
                String::from_utf8_lossy(&frame.payload).into_owned(),
            ));
        }
        if frame.header.kind == xs_proto::ControlKind::Hello as u8 {
            let hello: xs_proto::Hello = serde_json::from_slice(&frame.payload)
                .map_err(|error| Error::Companion(format!("Invalid tablet Hello: {error}")))?;
            if hello.protocol_version != xs_proto::PROTOCOL_VERSION {
                return Err(Error::Companion(format!("The tablet app speaks protocol v{} but this build speaks v{}. Reinstall the companion app.", hello.protocol_version, xs_proto::PROTOCOL_VERSION)));
            }
            return Ok(hello);
        }
    }
    Err(Error::Companion("Tablet never sent a Hello message".into()))
}

async fn read_hello_timeout(
    stream: &mut (impl tokio::io::AsyncRead + Unpin),
    timeout: Duration,
) -> Result<xs_proto::Hello> {
    tokio::time::timeout(timeout, read_hello(&mut FrameReader::new(stream)))
        .await
        .map_err(|_| {
            Error::Companion(
                "Tablet handshake timed out; unlock the tablet and tap Allow, then reconnect"
                    .into(),
            )
        })?
}

async fn negotiate_adb(
    control: &mut (impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin),
    mode: TransportMode,
) -> Result<xs_proto::Hello> {
    let hello = read_hello_timeout(control, Duration::from_secs(5)).await?;
    let selected = match mode.select(hello.transport) {
        Ok(selected) => selected,
        Err(message) => {
            FrameWriter::new(&mut *control)
                .write_frame(
                    xs_proto::Channel::Control,
                    xs_proto::ControlKind::Error as u8,
                    0,
                    0,
                    message.as_bytes(),
                )
                .await?;
            return Err(Error::Incompatible(message.into()));
        }
    };
    let request = serde_json::json!({ "transport": mode });
    FrameWriter::new(&mut *control)
        .write_frame(
            xs_proto::Channel::Control,
            xs_proto::ControlKind::HelloRequest as u8,
            0,
            0,
            request.to_string().as_bytes(),
        )
        .await?;
    // Wait for Android to acknowledge before connecting media channels
    // or switching USB functions. A mismatch is also visible on Android.
    let confirmed = read_hello_timeout(control, Duration::from_secs(5)).await?;
    if confirmed.transport != hello.transport {
        return Err(Error::Companion(
            "Tablet changed connection methods during negotiation; reconnect".into(),
        ));
    }
    if selected == TransportMode::Accessory {
        return Err(Error::SwitchToAccessory);
    }
    Ok(confirmed)
}

impl Transport {
    /// Full connect sequence: find the device, make sure the companion app is
    /// installed and running, forward the ports, and connect all three channels.
    ///
    /// `apk` is optional -- when present and newer than what is installed, it is
    /// pushed automatically so the app and host can never drift out of sync.
    pub async fn connect(apk: Option<&Path>, apk_version: u32) -> Result<Self> {
        Self::connect_with_mode(apk, apk_version, TransportMode::Auto).await
    }
    pub async fn connect_with_mode(
        apk: Option<&Path>,
        apk_version: u32,
        mode: TransportMode,
    ) -> Result<Self> {
        Self::connect_with_mode_and_progress(apk, apk_version, mode, |_| {}).await
    }

    pub async fn connect_with_mode_and_progress(
        apk: Option<&Path>,
        apk_version: u32,
        mode: TransportMode,
        progress: impl Fn(&str),
    ) -> Result<Self> {
        if fake_tablet_requested() {
            return Self::connect_fake().await;
        }
        // An explicit ADB choice must not change the tablet's USB function.
        if mode == TransportMode::Adb {
            return Self::connect_adb(apk, apk_version, mode).await;
        }
        // ADB control is also a discovery channel when either selector only
        // permits accessory. This lets us detect incompatible selections before
        // switching the device's USB mode or waiting for video sockets.
        match Self::connect_adb(apk, apk_version, mode).await {
            Ok(t) => Ok(t),
            Err(Error::SwitchToAccessory) => Self::connect_accessory(mode, &progress).await,
            Err(e)
                if matches!(
                    &e,
                    Error::Adb(
                        adb::Error::NoDevice
                            | adb::Error::Unauthorized(_)
                            | adb::Error::AdbNotFound
                    )
                ) =>
            {
                // Automatic and accessory choices permit accessory discovery.
                // An explicit ADB selector returned before reaching this fallback.
                match Self::connect_accessory(mode, &progress).await {
                    Err(Error::Adb(adb::Error::NoDevice)) => Err(e),
                    result => result,
                }
            }
            Err(e) => Err(e),
        }
    }
    async fn connect_accessory(mode: TransportMode, progress: &impl Fn(&str)) -> Result<Self> {
        let mut transport = aoa::connect(mode).await?;
        let cleanup = transport.teardown_handle();
        let mut guard = PendingConnection::new(cleanup.clone());
        progress("Tap Allow on your tablet to check USB connection methods…");
        let result = async {
            let hello = read_hello_timeout(&mut transport.control, Duration::from_secs(60)).await?;
            mode.select_for_link(hello.transport, TransportMode::Accessory)
                .map_err(|message| Error::Incompatible(message.into()))?;
            transport.hello = Some(hello);
            Ok(transport)
        }
        .await;
        if result.is_err() {
            cleanup.disconnect().await;
        }
        guard.disarm();
        result
    }

    async fn connect_adb(
        apk: Option<&Path>,
        apk_version: u32,
        mode: TransportMode,
    ) -> Result<Self> {
        if fake_tablet_requested() {
            return Self::connect_fake().await;
        }
        let adb = Adb::find()?;
        let device = adb.require_device().await?;
        info!(device = %device.display_name(), serial = %device.serial, "tablet found");

        if let Some(apk) = apk {
            if !apk.is_file() {
                return Err(Error::Companion(format!(
                    "APK does not exist: {}",
                    apk.display()
                )));
            }
            ensure_app_installed(&adb, &device.serial, apk, apk_version).await?;
        } else if adb
            .package_version(&device.serial, PACKAGE)
            .await?
            .is_none_or(|version| version < apk_version)
        {
            return Err(Error::Companion("Companion app is missing or older than this host. Build it with scripts/install.sh --build-apk, or bundle an APK with --apk PATH.".into()));
        }
        let cleanup = TransportHandle {
            device: device.clone(),
            adb: adb.clone(),
            simulated: false,
            accessory: None,
        };
        let mut guard = PendingConnection::new(cleanup.clone());
        let result = async {
            // Forward first: the app needs somewhere to be reached even though it is
            // the one listening.
            adb.forward(&device.serial, ports::CONTROL, sockets::CONTROL)
                .await?;
            adb.forward(&device.serial, ports::VIDEO, sockets::VIDEO)
                .await?;
            adb.forward(&device.serial, ports::CAMERA, sockets::CAMERA)
                .await?;

            // Restart the activity so we always talk to a fresh instance rather than
            // one left over from a previous run with stale sockets.
            adb.force_stop(&device.serial, PACKAGE).await;
            adb.start_activity(&device.serial, ACTIVITY).await?;

            // Control first, and only once it has actually spoken -- see
            // `connect_once_listening` for why connecting is not enough.
            let mut control = connect_once_listening(ports::CONTROL).await?;
            let hello = negotiate_adb(&mut control, mode).await?;
            let video = connect_with_retry(ports::VIDEO).await?;
            let camera = connect_with_retry(ports::CAMERA).await?;
            info!("all three channels connected");

            Ok(Self {
                device,
                hello: Some(hello),
                control: Box::new(control),
                video: Box::new(video),
                camera: Box::new(camera),
                adb,
                simulated: false,
                accessory: None,
            })
        }
        .await;
        if result.is_err() {
            cleanup.disconnect().await;
        }
        guard.disarm();
        result
    }

    /// Connects to a stand-in tablet already listening on the three ports.
    ///
    /// No adb, no device, no APK -- just three TCP connections. Used by the
    /// `fake_tablet` example to exercise the whole host pipeline on one machine.
    async fn connect_fake() -> Result<Self> {
        info!("{FAKE_TABLET_ENV} is set: connecting to a local stand-in, not a real tablet");
        let control = connect_once_listening(ports::CONTROL).await?;
        let video = connect_with_retry(ports::VIDEO).await?;
        let camera = connect_with_retry(ports::CAMERA).await?;
        Ok(Self {
            hello: None,
            device: Device {
                serial: "fake".into(),
                state: DeviceState::Ready,
                model: Some("Simulated_Tablet".into()),
            },
            control: Box::new(control),
            video: Box::new(video),
            camera: Box::new(camera),
            adb: Adb::find().unwrap_or_else(|_| Adb::none()),
            simulated: true,
            accessory: None,
        })
    }

    pub fn is_accessory(&self) -> bool {
        self.accessory.is_some()
    }

    /// A cleanup handle usable if host setup fails after transport connection.
    pub fn teardown_handle(&self) -> TransportHandle {
        TransportHandle {
            device: self.device.clone(),
            adb: self.adb.clone(),
            simulated: self.simulated,
            accessory: self.accessory.clone(),
        }
    }

    /// Splits into a teardown handle and the three sockets, so each can be moved
    /// into its own task.
    pub fn split(
        self,
    ) -> (
        TransportHandle,
        TransportStream,
        TransportStream,
        TransportStream,
    ) {
        (
            TransportHandle {
                device: self.device,
                adb: self.adb,
                simulated: self.simulated,
                accessory: self.accessory.clone(),
            },
            self.control,
            self.video,
            self.camera,
        )
    }

    /// Removes the port forwards. Called on teardown; failures are logged only.
    pub async fn disconnect(&self) {
        self.teardown_handle().disconnect().await;
    }
}

/// Installs the companion app if missing or out of date.
async fn ensure_app_installed(
    adb: &Adb,
    serial: &str,
    apk: &Path,
    bundled_version: u32,
) -> Result<()> {
    let checked = tokio::process::Command::new("python3")
        .arg("-c")
        .arg(include_str!("../../../scripts/check-apk.py"))
        .arg(apk)
        .arg(bundled_version.to_string())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| {
            Error::Companion(format!(
                "Could not verify companion APK (Python 3 required): {e}"
            ))
        })?;
    if !checked.status.success() {
        return Err(Error::Companion(
            String::from_utf8_lossy(&checked.stderr).trim().into(),
        ));
    }
    match adb.package_version(serial, PACKAGE).await? {
        Some(installed) if installed >= bundled_version => {
            debug!(installed, "companion app is up to date");
        }
        Some(installed) => {
            info!(
                installed,
                bundled = bundled_version,
                "upgrading companion app"
            );
            adb.install(serial, apk).await?;
            verify_installed_version(adb, serial, bundled_version).await?;
        }
        None => {
            info!("companion app not installed, installing");
            adb.install(serial, apk).await?;
            verify_installed_version(adb, serial, bundled_version).await?;
        }
    }
    Ok(())
}

async fn verify_installed_version(adb: &Adb, serial: &str, expected: u32) -> Result<()> {
    let actual = adb.package_version(serial, PACKAGE).await?;
    if actual != Some(expected) {
        return Err(Error::Companion(format!("The companion upgrade did not install version {expected} (found {actual:?}). Install a matching APK and reconnect.")));
    }
    Ok(())
}

/// Connects, and does not return until the peer has proven it is really there.
///
/// This exists because of a genuinely misleading `adb forward` behaviour: adb
/// accepts the *local* TCP connection whether or not anything is listening on the
/// device, and only then tries to open the remote socket. If that fails it simply
/// closes the connection. So `TcpStream::connect` succeeds on the very first try
/// even when the companion app has not finished starting, and the failure surfaces
/// milliseconds later as an unexplained EOF midway through the handshake.
///
/// Retrying on connection-refused therefore never fires. The only reliable signal
/// is bytes: the app writes `Hello` immediately on accept, so we peek for one byte
/// and treat silence or EOF as "not up yet".
async fn connect_once_listening(port: u16) -> Result<TcpStream> {
    /// How long to give the peer to say something before assuming it is a phantom.
    const PROBE_TIMEOUT: Duration = Duration::from_millis(400);

    let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
    let mut attempts = 0u32;
    loop {
        if let Ok(stream) = TcpStream::connect(("127.0.0.1", port)).await {
            let _ = stream.set_nodelay(true);
            let mut probe = [0u8; 1];
            // peek leaves the byte in the socket buffer for the real reader.
            match tokio::time::timeout(PROBE_TIMEOUT, stream.peek(&mut probe)).await {
                Ok(Ok(n)) if n > 0 => {
                    debug!(port, attempts, "control channel connected and talking");
                    return Ok(stream);
                }
                // n == 0 is EOF: adb's phantom accept. Anything else means the app
                // is up but silent, which for the control channel also means not ready.
                _ => {}
            }
        }
        attempts += 1;
        if tokio::time::Instant::now() >= deadline {
            return Err(Error::ConnectTimeout { port });
        }
        tokio::time::sleep(CONNECT_RETRY_DELAY).await;
    }
}

/// Connects to a forwarded port for a channel the peer does not speak on first.
///
/// Only safe to use *after* [`connect_once_listening`] has confirmed the app is
/// running, since it cannot distinguish a real connection from adb's phantom accept.
async fn connect_with_retry(port: u16) -> Result<TcpStream> {
    let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
    let mut attempts = 0u32;
    loop {
        match TcpStream::connect(("127.0.0.1", port)).await {
            Ok(stream) => {
                // Disable Nagle: we already batch a frame into one write, and
                // waiting to coalesce would add latency to every touch event.
                if let Err(e) = stream.set_nodelay(true) {
                    warn!(error = %e, "could not disable Nagle on port {port}");
                }
                debug!(port, attempts, "channel connected");
                return Ok(stream);
            }
            Err(_) if tokio::time::Instant::now() < deadline => {
                attempts += 1;
                tokio::time::sleep(CONNECT_RETRY_DELAY).await;
            }
            Err(_) => return Err(Error::ConnectTimeout { port }),
        }
    }
}

#[cfg(test)]
mod negotiation_tests {
    use super::*;
    use xs_proto::{Channel, ControlKind};

    fn hello(mode: TransportMode) -> xs_proto::Hello {
        serde_json::from_value(serde_json::json!({
            "protocol_version": xs_proto::PROTOCOL_VERSION,
            "transport": mode, "device_name": "Tablet", "device_id": "test-id",
            "android_release": "16", "width": 1920, "height": 1200,
            "density_dpi": 240, "refresh_rate": 60, "cameras": []
        }))
        .unwrap()
    }

    async fn send_hello(stream: &mut tokio::io::DuplexStream, mode: TransportMode) {
        FrameWriter::new(stream)
            .write_frame(
                Channel::Control,
                ControlKind::Hello as u8,
                0,
                0,
                &serde_json::to_vec(&hello(mode)).unwrap(),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn adb_discovery_negotiates_every_selector_pair_and_reports_mismatch_to_android() {
        for host in [
            TransportMode::Auto,
            TransportMode::Adb,
            TransportMode::Accessory,
        ] {
            for device in [
                TransportMode::Auto,
                TransportMode::Adb,
                TransportMode::Accessory,
            ] {
                let (mut client, mut tablet) = tokio::io::duplex(4096);
                let peer = tokio::spawn(async move {
                    send_hello(&mut tablet, device).await;
                    let request = FrameReader::new(&mut tablet).read_frame().await.unwrap();
                    assert_eq!(request.header.channel, Channel::Control);
                    if host.select(device).is_err() {
                        assert_eq!(request.header.kind, ControlKind::Error as u8);
                        assert!(String::from_utf8_lossy(&request.payload)
                            .starts_with("Incompatible connection methods:"));
                    } else {
                        assert_eq!(request.header.kind, ControlKind::HelloRequest as u8);
                        let json: serde_json::Value =
                            serde_json::from_slice(&request.payload).unwrap();
                        assert_eq!(json["transport"], serde_json::to_value(host).unwrap());
                        send_hello(&mut tablet, device).await;
                    }
                });
                let result = negotiate_adb(&mut client, host).await;
                match host.select(device) {
                    Ok(TransportMode::Adb) => assert_eq!(result.unwrap().transport, device),
                    Ok(TransportMode::Accessory) => {
                        assert!(matches!(result, Err(Error::SwitchToAccessory)))
                    }
                    Err(_) => assert!(matches!(result, Err(Error::Incompatible(_)))),
                    _ => panic!("selection must resolve to one concrete method"),
                }
                peer.await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn negotiation_rejects_changed_selection_in_acknowledgement() {
        let (mut client, mut tablet) = tokio::io::duplex(4096);
        let peer = tokio::spawn(async move {
            send_hello(&mut tablet, TransportMode::Auto).await;
            FrameReader::new(&mut tablet).read_frame().await.unwrap();
            send_hello(&mut tablet, TransportMode::Accessory).await;
        });
        assert!(negotiate_adb(&mut client, TransportMode::Auto)
            .await
            .unwrap_err()
            .to_string()
            .contains("changed connection methods"));
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn handshake_preserves_peer_error_instead_of_waiting_for_timeout() {
        let (mut client, mut tablet) = tokio::io::duplex(4096);
        FrameWriter::new(&mut tablet)
            .write_frame(
                Channel::Control,
                ControlKind::Error as u8,
                0,
                0,
                b"Incompatible connection methods: test",
            )
            .await
            .unwrap();
        assert_eq!(
            read_hello_timeout(&mut client, Duration::from_millis(100))
                .await
                .unwrap_err()
                .to_string(),
            "Incompatible connection methods: test"
        );
    }
}
