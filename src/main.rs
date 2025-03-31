#![allow(dead_code)]

use std::{fmt::Write, path::Path, process::Stdio, str::FromStr, sync::Arc};

use anyhow::{Context, Result};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{self, Child, ChildStdin, ChildStdout},
    signal,
    sync::{Mutex, mpsc},
    task,
};
use tracing::{debug, error, level_filters::LevelFilter, trace};

pub struct Engine {
    child: Child,
    tx: mpsc::Sender<String>,
    rx: mpsc::Receiver<String>,
    state: State,
}

async fn writer(mut stdin: ChildStdin, mut rx: mpsc::Receiver<String>) -> Result<()> {
    while let Some(cmd) = rx.recv().await {
        trace!("-> {cmd}");
        stdin.write_all(cmd.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
    }
    Ok(())
}

async fn reader(stdout: ChildStdout, tx: mpsc::Sender<String>) -> Result<()> {
    let mut reader = BufReader::new(stdout).lines();
    while let Some(line) = reader.next_line().await? {
        trace!("<- {line}");
        _ = tx.send(line).await
    }
    Ok(())
}

#[derive(Debug)]
enum Score {
    Cp(i32),
    Mate(i32),
}

impl Default for Score {
    fn default() -> Self {
        Self::Cp(0)
    }
}

#[derive(Debug, Default)]
struct Info {
    /// The depth of the search, which is the number of half-moves the engine is looking ahead.
    depth: u32,
    /// The selective depth, which indicates the deepest point the search has reached in some lines.
    seldepth: u32,
    /// The number of principal variations (PVs) being considered. In this case, only the best move (single PV) is being reported.
    multipv: u32,
    /// The evaluation score of the position in centipawns (1/100th of a pawn). Positive values favor White, and negative values favor Black.
    score: Score,
    wdl: (u64, u64, u64),
    /// The number of positions (nodes) the engine has evaluated so far.
    nodes: u64,
    /// Nodes per second, which indicates the speed of the engine's search.
    nps: u64,
    /// The percentage of the hash table used.
    hashfull: u32,
    /// The number of times a position was found in the tablebases.
    tbhits: u64,
    /// The time in milliseconds the engine has spent on this search.
    time: u64,
    /// The principal variation, which is the sequence of moves the engine considers best from the current position.
    pv: Vec<String>,
}

#[derive(Debug)]
struct BestMove {
    best: String,
    ponder: String,
}

#[derive(Debug)]
enum Search {
    Info(Info),
    BestMove(BestMove),
}

enum State {
    Init,
    Ready,
    Search,
}

impl Engine {
    fn new(path: impl AsRef<Path>) -> Result<Self> {
        let mut child = process::Command::new(path.as_ref())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;

        let stdin = child.stdin.take().context("failed to open stdin")?;
        let stdout = child.stdout.take().context("failed to open stdout")?;

        let (input_tx, input_rx) = mpsc::channel(32);
        let (output_tx, output_rx) = mpsc::channel(32);

        task::spawn(async move {
            if let Err(e) = writer(stdin, input_rx).await {
                error!(cause = %e, "writer error");
            }
        });

        task::spawn(async move {
            if let Err(e) = reader(stdout, output_tx).await {
                error!(cause = %e, "reader error");
            }
        });

        Ok(Self {
            child,
            tx: input_tx,
            rx: output_rx,
            state: State::Init,
        })
    }

    async fn send(&self, cmd: impl Into<String>) {
        self.tx.send(cmd.into()).await.expect("sender died");
    }

    async fn recv(&mut self) -> String {
        self.rx.recv().await.expect("reader died")
    }

    async fn uci(&mut self) {
        self.send("uci").await;
        loop {
            let line = self.recv().await;
            if line.starts_with("uciok") {
                break;
            }
        }
    }

    async fn isready(&mut self) {
        self.send("isready").await;
        loop {
            if self.recv().await == "readyok" {
                self.state = State::Ready;
                break;
            }
        }
    }

    async fn opt(&mut self, name: &str, value: &str) {
        self.send(format!("setoption name {name} value {value}"))
            .await;
    }

    async fn opts(&mut self, opts: &[(&str, &str)]) {
        for (name, value) in opts {
            self.opt(name, value).await;
        }
    }

    async fn go(&mut self) {}
}

const FEN_MATE: &str = "r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 4 4";

#[derive(Debug, Default)]
struct Go {
    position: Option<String>,
    moves: Vec<String>,
    depth: u32,
}

impl Go {
    fn new() -> Self {
        Self {
            depth: 10,
            ..Default::default()
        }
    }

    fn fen(mut self, position: impl Into<String>) -> Self {
        self.position = Some(position.into());
        self
    }

    fn moves(mut self, moves: &[impl AsRef<str>]) -> Self {
        for mv in moves {
            self.moves.push(mv.as_ref().into());
        }
        self
    }

    fn depth(mut self, depth: u32) -> Self {
        self.depth = depth;
        self
    }

    fn execute(self, engine: Arc<Mutex<Engine>>) -> Searcher {
        let (tx, rx) = mpsc::channel(100);
        task::spawn(async move {
            debug!(?self);
            if let Err(e) = go(engine, self, tx).await {
                error!(cause = %e, "go error");
            }
        });
        Searcher { rx }
    }
}

struct Searcher {
    rx: mpsc::Receiver<Search>,
}

impl Searcher {
    async fn next(&mut self) -> Option<Search> {
        self.rx.recv().await
    }
}

async fn go(engine: Arc<Mutex<Engine>>, job: Go, tx: mpsc::Sender<Search>) -> Result<()> {
    let mut position = "position".to_string();
    match &job.position {
        None => write!(&mut position, " startpos")?,
        Some(fen) => write!(&mut position, " fen {fen}")?,
    };

    if !job.moves.is_empty() {
        write!(&mut position, " moves {}", job.moves.join(" "))?;
    }

    let mut engine = engine.lock().await;
    engine.send(position).await;
    engine.send(format!("go depth {}", job.depth)).await;
    engine.state = State::Search;

    loop {
        let line = engine.recv().await;
        if line.starts_with("info depth") {
            let info = line.parse::<Info>()?;
            _ = tx.send(Search::Info(info)).await;
        } else if line.starts_with("bestmove") {
            let bestmove = line.parse::<BestMove>()?;
            _ = tx.send(Search::BestMove(bestmove)).await;
            engine.state = State::Ready;
            break;
        }
    }

    Ok(())
}

fn parse_info(line: &str) -> Result<Info> {
    let mut info = Info::default();
    let mut parts = line.split_whitespace();

    while let Some(part) = parts.next() {
        match part {
            "depth" => info.depth = parts.next().context("no depth")?.parse()?,
            "seldepth" => info.seldepth = parts.next().context("no seldepth")?.parse()?,
            "multipv" => info.multipv = parts.next().context("no multipv")?.parse()?,
            "score" => match parts.next().context("no score")? {
                "cp" => info.score = Score::Cp(parts.next().context("no cp")?.parse()?),
                "mate" => info.score = Score::Mate(parts.next().context("no mate")?.parse()?),
                other => eprintln!("Unkwown score: {other}"),
            },
            "wdl" => {
                info.wdl.0 = parts.next().context("no win %")?.parse()?;
                info.wdl.1 = parts.next().context("no draw %")?.parse()?;
                info.wdl.2 = parts.next().context("no lose %")?.parse()?;
            }
            "nodes" => info.nodes = parts.next().context("no nodes")?.parse()?,
            "nps" => info.nps = parts.next().context("no nps")?.parse()?,
            "hashfull" => info.hashfull = parts.next().context("no hashfull")?.parse()?,
            "tbhits" => info.tbhits = parts.next().context("no tbhits")?.parse()?,
            "time" => info.time = parts.next().context("no time")?.parse()?,
            "pv" => {
                while let Some(mv) = parts.next() {
                    info.pv.push(mv.into());
                }
            }
            _ => (),
        };
    }

    Ok(info)
}

impl FromStr for Info {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        parse_info(s)
    }
}

fn parse_bestmove(line: &str) -> Result<BestMove> {
    let parts = line.split_whitespace().collect::<Vec<_>>();
    Ok(BestMove {
        best: parts[1].into(),
        ponder: parts[3].into(),
    })
}

impl FromStr for BestMove {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        parse_bestmove(s)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(LevelFilter::TRACE)
        .without_time()
        .with_target(false)
        .compact()
        .init();

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
            .moves(&["d2d4", "g8f6", "c2c4", "e7e6", "g1f3", "d7d5"])
            .depth(32)
            .execute(engine);

        while let Some(search) = searcher.next().await {
            match search {
                Search::Info(info) => debug!(?info),
                Search::BestMove(bestmove) => debug!(?bestmove),
            };
        }
    }

    // _ = signal::ctrl_c().await;

    Ok(())
}
