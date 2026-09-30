//! Real-device lifecycle smoke test. Uses the installed companion, never installs an APK.
//! cargo run --release -p xs-core --example device_cycle -- 3 10
use std::time::Duration;
use xs_core::{Command, Event, SessionConfig, State};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();
    let args: Vec<_> = std::env::args().collect();
    let cycles: usize = args.get(1).map(|s| s.parse()).transpose()?.unwrap_or(3);
    let seconds: u64 = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(10);
    let config = SessionConfig {
        scale: std::env::var("XS_TEST_SCALE")
            .ok()
            .map(|s| s.parse())
            .transpose()?
            .unwrap_or(1.5),
        mode: if std::env::var_os("XS_TEST_MIRROR").is_some() {
            xs_core::DisplayMode::Mirror
        } else {
            xs_core::DisplayMode::Extend
        },
        ..Default::default()
    };
    let engine = xs_core::spawn(config);
    let mut events = engine.subscribe();
    let result = async {
        for cycle in 1..=cycles {
            engine.send(Command::Connect);
            tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    match events.recv().await? {
                        Event::State(State::Streaming {
                            width,
                            height,
                            encoder,
                            ..
                        }) => {
                            println!("cycle {cycle}: streaming {width}x{height} ({encoder})");
                            break;
                        }
                        Event::State(State::Failed { message }) => anyhow::bail!("{message}"),
                        Event::State(State::NoTablet) => anyhow::bail!("no tablet"),
                        Event::State(State::Unauthorized { .. }) => {
                            anyhow::bail!("tablet unauthorized")
                        }
                        _ => {}
                    }
                }
                anyhow::Ok(())
            })
            .await??;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
            let mut frames = 0;
            let mut decoded = 0;
            let mut induced_loss = false;
            let mut saw_reconnect = false;
            while let Ok(event) = tokio::time::timeout_at(deadline, events.recv()).await {
                match event? {
                    Event::Stats(stats) => {
                        frames = stats.frames_encoded;
                        decoded = stats.frames_decoded;
                        if !induced_loss
                            && decoded > 0
                            && std::env::var_os("XS_TEST_LOSS").is_some()
                        {
                            induced_loss = true;
                            let status = tokio::process::Command::new("adb")
                                .args(["-d", "shell", "am", "force-stop", xs_transport::PACKAGE])
                                .status()
                                .await?;
                            anyhow::ensure!(
                                status.success(),
                                "could not induce companion disconnect"
                            );
                            println!("cycle {cycle}: induced companion disconnect");
                        }
                        println!("cycle {cycle}: {stats:?}");
                    }
                    Event::State(State::Failed { message }) => anyhow::bail!("{message}"),
                    Event::State(state) => {
                        if induced_loss && matches!(state, State::Streaming { .. }) {
                            saw_reconnect = true;
                        }
                        println!("cycle {cycle}: {state:?}");
                    }
                    _ => {}
                }
            }
            anyhow::ensure!(
                !induced_loss || saw_reconnect,
                "did not automatically reconnect"
            );
            anyhow::ensure!(decoded > 0, "no decoded frames in cycle {cycle}");
            anyhow::ensure!(frames > 0, "no encoded frames in cycle {cycle}");
            engine.send(Command::Disconnect);
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if matches!(events.recv().await?, Event::State(State::Idle)) {
                        break;
                    }
                }
                anyhow::Ok(())
            })
            .await??;
            tokio::time::sleep(Duration::from_millis(1500)).await;
        }
        anyhow::Ok(())
    }
    .await;
    engine.shutdown().await;
    result?;
    println!("PASS: {cycles} connect/disconnect cycles");
    Ok(())
}
