//! Real-device regression for live CPU/GPU selection and unavailable drivers.
//! Close the normal app before running. Uses the installed companion only.
use std::time::Duration;
use xs_core::{Command, EncoderSelection, EncodingMode, Event, State};

async fn wait_streaming(
    events: &mut tokio::sync::broadcast::Receiver<Event>,
) -> anyhow::Result<String> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match events.recv().await? {
                Event::State(State::Streaming { encoder, .. }) => return Ok(encoder),
                Event::State(State::Failed { message }) => anyhow::bail!("{message}"),
                _ => {}
            }
        }
    })
    .await?
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,xs_video=debug")
        .init();
    let options = xs_core::available_encoders();
    anyhow::ensure!(options.iter().any(|o| o.is_gpu()), "no GPU encoder to test");
    let engine = xs_core::spawn(xs_core::SessionConfig {
        scale: 1.75,
        ..Default::default()
    });
    let mut events = engine.subscribe();
    let result = async {
        engine.send(Command::Connect);
        println!("Automatic: {}", wait_streaming(&mut events).await?);
        for mode in [EncodingMode::Cpu, EncodingMode::Gpu] {
            engine.send(Command::SetEncoder(EncoderSelection {
                mode,
                factory: None,
            }));
            println!("{mode:?}: {}", wait_streaming(&mut events).await?);
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        // Exercise every detected implementation on this machine.
        for option in options {
            engine.send(Command::SetEncoder(EncoderSelection {
                mode: if option.is_gpu() {
                    EncodingMode::Gpu
                } else {
                    EncodingMode::Cpu
                },
                factory: Some(option.factory.clone()),
            }));
            let encoder = wait_streaming(&mut events).await?;
            anyhow::ensure!(
                encoder == option.label,
                "requested encoder was silently substituted"
            );
            println!("Pinned {}: {encoder}", option.factory);
        }
        engine.send(Command::SetEncoder(EncoderSelection {
            mode: EncodingMode::Gpu,
            factory: Some("missing-encoder-for-test".into()),
        }));
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Event::State(State::Failed { message }) = events.recv().await? {
                    anyhow::ensure!(
                        message.contains("unavailable"),
                        "unexpected error: {message}"
                    );
                    println!("Missing driver: {message}");
                    break;
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        engine.send(Command::SetEncoder(EncoderSelection::default()));
        engine.send(Command::Connect);
        println!("Recovered: {}", wait_streaming(&mut events).await?);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    engine.shutdown().await;
    result
}
