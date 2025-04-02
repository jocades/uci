use std::process::Stdio;
use std::{fmt::Write, path::Path};

use anyhow::Result;
use tokio::sync::oneshot;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin},
    sync::mpsc,
    task,
};
use uci::search::{BestMove, Info, Search};

#[derive(Debug)]
enum Command {
    IsReady(oneshot::Sender<()>),
    SetOption { name: String, value: String },
    Go { job: Go, tx: mpsc::Sender<Search> },
}

struct Engine {
    child: Child,
    rx: mpsc::Receiver<Command>,
}

impl Engine {
    async fn run(&mut self) -> Result<()> {
        let mut stdin = self.child.stdin.take().unwrap();
        let stdout = self.child.stdout.take().unwrap();

        let mut reader = BufReader::new(stdout).lines();

        stdin.write_all(b"uci\n").await?;
        while let Some(line) = reader.next_line().await? {
            if line == "uciok" {
                break;
            }
        }

        while let Some(cmd) = self.rx.recv().await {
            println!("{cmd:?}");
            match cmd {
                Command::IsReady(ack) => {
                    stdin.write_all(b"isready\n").await?;
                    while let Some(line) = reader.next_line().await? {
                        if line == "readyok" {
                            _ = ack.send(());
                            break;
                        }
                    }
                }
                Command::SetOption { name, value } => todo!(),
                Command::Go { job, tx: resp } => {
                    let mut position = "position".to_string();

                    match &job.position {
                        None => write!(&mut position, " startpos")?,
                        Some(fen) => write!(&mut position, " fen {fen}")?,
                    };
                    if !job.moves.is_empty() {
                        write!(&mut position, " moves {}", job.moves.join(" "))?;
                    }
                    position.push('\n');

                    stdin.write_all(position.as_bytes()).await?;
                    stdin
                        .write_all(format!("go depth {}\n", job.depth).as_bytes())
                        .await?;

                    while let Some(line) = reader.next_line().await.unwrap() {
                        if line.starts_with("info depth") {
                            let info = line.parse::<Info>().unwrap();
                            _ = resp.send(Search::Info(info)).await;
                        } else if line.starts_with("bestmove") {
                            let bestmove = line.parse::<BestMove>().unwrap();
                            _ = resp.send(Search::BestMove(bestmove)).await;
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
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

    pub async fn execute(self, engine: &Handle) -> Result<Searcher> {
        engine.go(self).await
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

pub struct Handle {
    tx: mpsc::Sender<Command>,
}

impl Handle {
    async fn isready(&self) -> Result<()> {
        let (ack, syn) = oneshot::channel();
        self.tx.send(Command::IsReady(ack)).await?;
        syn.await?;
        Ok(())
    }

    async fn go(&self, job: Go) -> Result<Searcher> {
        let (tx, rx) = mpsc::channel(100);
        self.tx.send(Command::Go { job, tx }).await?;
        Ok(Searcher { rx })
    }
}

fn spawn() -> Result<Handle> {
    let (tx, rx) = mpsc::channel(100);
    let child = tokio::process::Command::new("stockfish")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;

    let mut engine = Engine { child, rx };
    task::spawn(async move {
        if let Err(e) = engine.run().await {
            eprintln!("engine error: {e}");
        }
    });

    Ok(Handle { tx })
}

async fn search(mut searcher: Searcher) {
    while let Some(search) = searcher.next().await {
        match search {
            Search::Info(info) => eprintln!("{info:?}"),
            Search::BestMove(bestmove) => eprintln!("{bestmove:?}"),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let engine = spawn()?;
    engine.isready().await?;

    let searcher = Go::new()
        .moves(&["d2d4", "g8f6", "c2c4", "e7e6", "g2g3"])
        .depth(5)
        .execute(&engine)
        .await?;

    search(searcher).await;

    let job = Go::new().moves(&["d2d4", "g8f6"]).depth(10);
    let searcher = engine.go(job).await?;

    search(searcher).await;

    // let mut rx = engine.go().await?;
    //
    // while let Some(search) = rx.recv().await {
    //     println!("{search:?}");
    // }

    // let mut child = tokio::process::Command::new("stockfish")
    //     .stdin(Stdio::piped())
    //     .stdout(Stdio::piped())
    //     .spawn()?;
    //
    // let mut stdin = child.stdin.take().unwrap();
    // let stdout = child.stdout.take().unwrap();
    //
    // let (input_tx, input_rx) = mpsc::channel(50);
    // task::spawn(async move {
    //     if let Err(e) = writer(stdin, input_rx).await {
    //         eprintln!("writer error: {e}");
    //     }
    // });
    //
    // let mut reader = BufReader::new(stdout).lines();
    //
    // input_tx.send("uci\n".into()).await?;
    // while let Some(line) = reader.next_line().await? {
    //     if line == "uciok" {
    //         break;
    //     }
    // }
    //
    // let opts = [("Threads", "8"), ("UCI_ShowWDL", "true")];
    // let cmd = opts.iter().fold(String::new(), |mut acc, (name, value)| {
    //     _ = writeln!(acc, "name {name} value {value}");
    //     acc
    // });
    // input_tx.send(cmd).await?;
    //
    // input_tx.send("isready\n".into()).await?;
    // while let Some(line) = reader.next_line().await? {
    //     if line == "readyok" {
    //         break;
    //     }
    // }
    //
    // input_tx
    //     .send("position startpos moves d2d4 g1f6\n".into())
    //     .await?;
    // input_tx.send("go depth 10\n".into()).await?;
    //
    // let (tx, mut rx) = mpsc::channel(100);
    // task::spawn(async move {
    //     while let Some(line) = reader.next_line().await.unwrap() {
    //         if line.starts_with("info depth") {
    //             let info = line.parse::<Info>().unwrap();
    //             _ = tx.send(Search::Info(info)).await;
    //         } else if line.starts_with("bestmove") {
    //             let bestmove = line.parse::<BestMove>().unwrap();
    //             _ = tx.send(Search::BestMove(bestmove)).await;
    //             break;
    //         }
    //     }
    // });
    //
    // while let Some(search) = rx.recv().await {
    //     println!("{search:?}");
    // }

    Ok(())
}
