use std::str::FromStr;

use anyhow::{Context, Result};

#[derive(Debug)]
pub enum Score {
    Cp(i32),
    Mate(i32),
}

impl Default for Score {
    fn default() -> Self {
        Self::Cp(0)
    }
}

#[derive(Debug, Default)]
pub struct Info {
    /// The depth of the search, which is the number of half-moves the engine is looking ahead.
    pub depth: u32,
    /// The selective depth, which indicates the deepest point the search has reached in some lines.
    pub seldepth: u32,
    /// The number of principal variations (PVs) being considered. In this case, only the best move (single PV) is being reported.
    pub multipv: u32,
    /// The evaluation score of the position in centipawns (1/100th of a pawn). Positive values favor White, and negative values favor Black.
    pub score: Score,
    pub wdl: (u64, u64, u64),
    /// The number of positions (nodes) the engine has evaluated so far.
    pub nodes: u64,
    /// Nodes per second, which indicates the speed of the engine's search.
    pub nps: u64,
    /// The percentage of the hash table used.
    pub hashfull: u32,
    /// The number of times a position was found in the tablebases.
    pub tbhits: u64,
    /// The time in milliseconds the engine has spent on this search.
    pub time: u64,
    /// The principal variation, which is the sequence of moves the engine considers best from the current position.
    pub pv: Vec<String>,
}

#[derive(Debug)]
pub struct BestMove {
    pub best: String,
    pub ponder: String,
}

#[derive(Debug)]
pub enum Search {
    Info(Info),
    BestMove(BestMove),
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
