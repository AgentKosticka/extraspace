//! Hardware smoke test: cargo run -p xs-core --example usb_reconnect -- [adb|accessory]
//! Creates and removes a real monitor. Keep the tablet unlocked for AOA consent.
use std::time::Duration;
use xs_core::{Command, Event, SessionConfig, State, TransportMode};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let mode = match std::env::args().nth(1).as_deref() {
        Some("accessory") => TransportMode::Accessory,
        _ => TransportMode::Adb,
    };
    let engine = xs_core::spawn(SessionConfig {
        transport: mode,
        ..Default::default()
    });
    let mut events = engine.subscribe();
    if std::env::args().nth(2).as_deref() == Some("cancel") {
        engine.send(Command::Connect);
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if let Ok(Event::State(State::Connecting { step })) = events.recv().await {
                    if step.contains("Tap Allow") {
                        break;
                    }
                }
            }
        })
        .await?;
        tokio::time::timeout(Duration::from_secs(3), engine.shutdown()).await?;
        println!("PASS cancellation while waiting for accessory consent");
        return Ok(());
    }
    let result = async {
        for round in 0..2 {
            engine.send(Command::Connect);
            let state = tokio::time::timeout(Duration::from_secs(90), async {
                loop {
                    if let Ok(Event::State(state)) = events.recv().await {
                        println!("STATE {round} {state:?}");
                        if matches!(state, State::Streaming { .. } | State::Failed { .. }) {
                            break state;
                        }
                    }
                }
            })
            .await?;
            anyhow::ensure!(
                matches!(state, State::Streaming { .. }),
                "stream did not start: {state:?}"
            );
            let mut decoded = false;
            let end = tokio::time::sleep(Duration::from_secs(15));
            tokio::pin!(end);
            loop {
                tokio::select! {
                    _ = &mut end => break,
                    event = events.recv() => match event {
                        Ok(Event::Stats(s)) => { decoded |= s.frames_decoded > 0; }
                        Ok(Event::State(State::Failed { message })) => anyhow::bail!("{message}"),
                        _ => {},
                    }
                }
            }
            anyhow::ensure!(decoded, "no decoded video reported in round {round}");
            engine.send(Command::Disconnect);
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if matches!(events.recv().await, Ok(Event::State(State::Idle))) {
                        break;
                    }
                }
            })
            .await?;
            println!("PASS round={round} decoded frames and clean disconnect");
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    engine.shutdown().await;
    result
}
