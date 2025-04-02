use std::{fmt::Write, path::Path, process::Stdio, sync::Arc};

use anyhow::{Context, Result};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{self, Child, ChildStdin, ChildStdout},
    sync::{Mutex, mpsc},
    task,
};
use tracing::{debug, error, trace};

use crate::search::{BestMove, Info, Search};

pub struct Engine {
    #[allow(unused)]
    child: Child,
    tx: mpsc::Sender<String>,
    rx: mpsc::Receiver<String>,
    state: State,
}

pub enum State {
    Init,
    Ready,
    Search,
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

impl Engine {
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
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

    pub async fn uci(&mut self) {
        self.send("uci").await;
        loop {
            let line = self.recv().await;
            if line.starts_with("uciok") {
                break;
            }
        }
    }

    pub async fn isready(&mut self) {
        self.send("isready").await;
        loop {
            if self.recv().await == "readyok" {
                self.state = State::Ready;
                break;
            }
        }
    }

    pub async fn opt(&mut self, name: &str, value: &str) {
        self.send(format!("setoption name {name} value {value}"))
            .await;
    }

    pub async fn opts(&mut self, opts: &[(&str, &str)]) {
        for (name, value) in opts {
            self.opt(name, value).await;
        }
    }
}

#[derive(Debug, Default)]
pub struct Go {
    position: Option<String>,
    moves: Vec<String>,
    depth: u32,
}

impl Go {
    pub fn new() -> Self {
        Self {
            depth: 10,
            ..Default::default()
        }
    }

    pub fn fen(mut self, position: impl Into<String>) -> Self {
        self.position = Some(position.into());
        self
    }

    pub fn moves(mut self, moves: &[impl AsRef<str>]) -> Self {
        for mv in moves {
            self.moves.push(mv.as_ref().into());
        }
        self
    }

    pub fn depth(mut self, depth: u32) -> Self {
        self.depth = depth;
        self
    }

    pub fn execute(self, engine: Arc<Mutex<Engine>>) -> Searcher {
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

pub struct Searcher {
    rx: mpsc::Receiver<Search>,
}

impl Searcher {
    pub async fn next(&mut self) -> Option<Search> {
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
