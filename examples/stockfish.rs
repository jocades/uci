use std::process::Stdio;
use std::time::Duration;
use std::{fmt::Write, path::Path};

use anyhow::Result;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout},
    sync::{mpsc, oneshot},
    task,
};
use uci::search::{BestMove, Info, Search};

#[derive(Debug)]
enum Command {
    IsReady(oneshot::Sender<()>),
    SetOption { name: String, value: String },
    Go { job: Go, tx: mpsc::Sender<Search> },
    Stop(oneshot::Sender<()>),
}

impl std::fmt::Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Command::IsReady(_) => write!(f, "isready"),
            Command::SetOption { name, value } => write!(f, "setoption {name}: {value}"),
            Command::Go { job, .. } => write!(f, "go depth: {}", job.depth),
            Command::Stop(_) => write!(f, "stop"),
        }
    }
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
        self.wait(&mut reader, "uciok").await?;

        while let Some(cmd) = self.rx.recv().await {
            println!("-> {cmd}");
            match cmd {
                Command::IsReady(ack) => {
                    self.isready(&mut stdin, &mut reader).await?;
                    _ = ack.send(());
                }
                Command::SetOption { name, value } => todo!(),
                Command::Go { job, tx } => {
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

                    while let Some(line) = reader.next_line().await? {
                        if line.starts_with("info depth") {
                            let info = line.parse::<Info>().unwrap();
                            _ = tx.send(Search::Info(info)).await;
                        } else if line.starts_with("bestmove") {
                            let bestmove = line.parse::<BestMove>().unwrap();
                            _ = tx.send(Search::BestMove(bestmove)).await;
                            break;
                        }
                    }
                }
                Command::Stop(ack) => {
                    stdin.write_all(b"stop\nisready\n").await?;
                    stdin.write_all(b"isready\n").await?;
                    self.wait(&mut reader, "readyok").await?;
                    _ = ack.send(());
                    eprintln!("stopped!");
                }
            }
        }

        Ok(())
    }

    async fn isready(
        &mut self,
        stdin: &mut ChildStdin,
        reader: &mut Lines<BufReader<ChildStdout>>,
    ) -> Result<()> {
        stdin.write_all(b"isready\n").await?;
        self.wait(reader, "readyok").await?;
        Ok(())
    }

    async fn wait(&self, reader: &mut Lines<BufReader<ChildStdout>>, kw: &str) -> Result<()> {
        while let Some(line) = reader.next_line().await? {
            if line == kw {
                break;
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

    async fn stop(&self) -> Result<()> {
        let (ack, syn) = oneshot::channel();
        self.tx.send(Command::Stop(ack)).await?;
        syn.await?;
        Ok(())
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
        // .moves(&["f2f3", "e7e5", "g2g4"])
        .depth(25)
        .execute(&engine)
        .await?;

    tokio::time::sleep(Duration::from_secs(2)).await;
    engine.stop().await?;

    // search(searcher).await;

    // let job = Go::new().moves(&["d2d4", "g8f6"]).depth(10);
    // let searcher = engine.go(job).await?;
    //
    // let timer = tokio::time::sleep(Duration::from_millis(500));
    // tokio::pin!(timer);
    //
    // search(searcher).await;

    // loop {
    //     tokio::select! {
    //         search = searcher.next() => eprintln!("{search:?}"),
    //         // () = &mut timer => {
    //         //     println!("timeout");
    //         //     engine.stop().await?;
    //         //     break;
    //         // }
    //
    //     }
    // }

    println!("done!");

    // search(searcher).await;

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
