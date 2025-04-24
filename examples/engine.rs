use std::{fmt::Write, path::Path, process::Stdio, time::Duration};

use anyhow::{Context, Result};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    select,
    sync::mpsc,
    time,
};
use tracing::{debug, error, trace};

use uci::search::{BestMove, Info, Search};

async fn writer(mut stdin: ChildStdin, mut rx: mpsc::Receiver<String>) -> Result<()> {
    while let Some(mut cmd) = rx.recv().await {
        trace!("-> {cmd}");
        cmd.push('\n');
        stdin.write_all(cmd.as_bytes()).await?;
        stdin.flush().await?;
    }
    Ok(())
}

async fn reader(stdout: ChildStdout, tx: mpsc::Sender<String>) -> Result<()> {
    let mut reader = BufReader::new(stdout).lines();
    while let Some(line) = reader.next_line().await? {
        trace!("<- {line}");
        tx.send(line).await?;
    }
    Ok(())
}

#[derive(Debug, Default)]
pub struct Go {
    fen: Option<String>,
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

    pub fn fen(mut self, fen: impl Into<String>) -> Self {
        self.fen = Some(fen.into());
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

    pub async fn execute(self, engine: &Engine) -> Result<()> {
        // engine.go(self).await
        todo!()
    }
}

pub struct Engine {
    _child: Child,
    tx: mpsc::Sender<String>,
    rx: mpsc::Receiver<String>,
}

impl Engine {
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let mut child = Command::new(path.as_ref())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;

        let stdin = child.stdin.take().context("failed to open stdin")?;
        let stdout = child.stdout.take().context("failed to open stdout")?;

        let (input_tx, input_rx) = mpsc::channel(32);
        tokio::spawn(async move {
            if let Err(e) = writer(stdin, input_rx).await {
                error!(cause = %e, "writer error");
            }
        });

        let (output_tx, output_rx) = mpsc::channel(32);
        // let output_rx = Arc::new(Mutex::new(output_rx));
        tokio::spawn(async move {
            if let Err(e) = reader(stdout, output_tx).await {
                error!(cause = %e, "reader error");
            }
        });

        Ok(Self {
            _child: child,
            tx: input_tx,
            rx: output_rx,
        })
    }

    #[tracing::instrument(level = "debug", skip(self))]
    pub async fn wait(&mut self, keyword: &str) {
        // let mut rx = self.rx.lock().await;
        while let Some(line) = self.rx.recv().await {
            if line == keyword {
                break;
            }
        }
    }

    pub async fn uci(&mut self) -> Result<()> {
        self.tx.send("uci".into()).await?;
        self.wait("uciok").await;
        Ok(())
    }

    pub async fn isready(&mut self) -> Result<()> {
        self.tx.send("isready".into()).await?;
        self.wait("readyok").await;
        debug!("READY");
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        self.tx.send("stop\nisready".into()).await?;
        self.wait("readyok").await;
        debug!("READY");
        Ok(())
    }

    pub async fn opts<O: std::fmt::Display>(&self, options: &[(O, O)]) -> Result<()> {
        let cmd = options.iter().fold(String::new(), |mut acc, (k, v)| {
            _ = writeln!(&mut acc, "setoption name {k} value {v}");
            acc
        });
        self.tx.send(cmd).await?;
        Ok(())
    }

    fn prepare(&self, job: Go) -> String {
        let mut cmd = "position".to_string();
        match &job.fen {
            None => _ = write!(&mut cmd, " startpos"),
            Some(fen) => _ = write!(&mut cmd, " fen {fen}"),
        };
        if !job.moves.is_empty() {
            _ = write!(&mut cmd, " moves {}", job.moves.join(" "));
        }
        cmd.push('\n');

        _ = writeln!(&mut cmd, "go depth {}", job.depth);

        cmd
    }

    pub async fn go(&mut self, job: Go) -> Result<(Info, BestMove)> {
        let cmd = self.prepare(job);
        self.tx.send(cmd).await?;

        let mut last: Option<Info> = None;
        let mut best: Option<BestMove> = None;

        while let Some(line) = self.rx.recv().await {
            match search(&line) {
                Some(Search::Info(info)) => last = Some(info),
                Some(Search::BestMove(b)) => {
                    best = Some(b);
                    break;
                }
                None => continue,
            };
        }

        Ok((last.unwrap(), best.unwrap()))
    }
}

fn search(line: &str) -> Option<Search> {
    if line.starts_with("info depth") {
        let info = line.parse::<uci::search::Info>().unwrap();
        return Some(Search::Info(info));
    }
    if line.starts_with("bestmove") {
        let best = line.parse::<uci::search::BestMove>().unwrap();
        return Some(Search::BestMove(best));
    }
    None
}

#[tokio::main]
async fn main() -> Result<()> {
    setup_logging();

    let mut engine = Engine::new("stockfish")?;
    let options = [("Threads", "8"), ("UCI_ShowWDL", "true"), ("MultiPV", "2")];

    engine.uci().await?;

    engine.opts(&options).await?;
    engine.isready().await?;

    let job = Go::new().moves(&["e2e4"]).depth(10);
    let cmd = engine.prepare(job);

    engine.tx.send(cmd).await?;

    let timer = time::sleep(Duration::from_secs(1));
    tokio::pin!(timer);

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
                debug!("timer done");
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
        .with_max_level(LevelFilter::TRACE)
        .without_time()
        .with_target(false)
        .compact()
        .init();
}
