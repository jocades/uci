use std::time::Duration;

use anyhow::Result;
use tokio::{select, time};

use uci::{
    engine::{Engine, Go, search},
    search::Search,
};

#[tokio::main]
async fn main() -> Result<()> {
    setup_logging();

    let mut engine = Engine::new("stockfish")?;
    let options = [("Threads", "8"), ("UCI_ShowWDL", "true"), ("MultiPV", "1")];

    engine.uci().await?;

    engine.opts(&options).await?;
    engine.isready().await?;

    let job = Go::new().moves(&["f2f3"]).depth(25);
    let cmd = engine.prepare(job);

    let timer = time::sleep(Duration::from_secs(1));
    tokio::pin!(timer);

    engine.tx.send(cmd).await?;
    loop {
        select! {
            Some(line) = engine.rx.recv() => match search(&line) {
                Some(Search::Info(info)) => {
                    tracing::info!(?info);
                },
                Some(Search::BestMove(best)) => {
                    tracing::info!(?best);
                    break;
                },
                None => continue,
            },
            _ = &mut timer => {
                engine.stop().await?;
                break;
            },
        }
    }

    Ok(())
}

fn setup_logging() {
    use tracing::level_filters::LevelFilter;

    tracing_subscriber::fmt()
        .with_max_level(LevelFilter::DEBUG)
        .without_time()
        .with_target(false)
        .compact()
        .init();
}
