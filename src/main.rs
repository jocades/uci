use std::sync::Arc;

use anyhow::Result;
use tokio::{signal, sync::Mutex};
use tracing::debug;

use uci::{
    FEN_MATE,
    engine::{Engine, Go},
    search::Search,
};

#[tokio::main]
async fn main() -> Result<()> {
    setup_logging();

    let engine = Arc::new(Mutex::new(Engine::new("stockfish")?));

    {
        let mut engine = engine.lock().await;
        engine.uci().await;
        engine
            .opts(&[("Threads", "8"), ("UCI_ShowWDL", "true"), ("MultiPV", "2")])
            .await;
        engine.isready().await;
    }

    {
        let engine = engine.clone();
        let mut searcher = Go::new()
            // .moves(&["d2d4", "g8f6", "c2c4", "e7e6", "g1f3", "d7d5"])
            .fen("rnbqr1k1/2p2ppp/pn2p3/bp1pP1NQ/3P4/P1PB4/1P3PPP/RNB2RK1 w - - 5 12")
            .depth(10)
            .execute(engine);

        while let Some(search) = searcher.next().await {
            match search {
                Search::Info(info) => debug!(?info),
                Search::BestMove(bestmove) => debug!(?bestmove),
            };
        }
    }

    _ = signal::ctrl_c().await;

    Ok(())
}

fn setup_logging() {
    use tracing::level_filters::LevelFilter;

    tracing_subscriber::fmt()
        .with_max_level(LevelFilter::TRACE)
        .without_time()
        .with_target(false)
        .compact()
        .init();
}
