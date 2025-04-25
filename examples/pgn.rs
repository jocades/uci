use std::sync::Arc;

use anyhow::Result;
use pgn_reader::{BufferedReader, RawHeader, SanPlus, Skip, Visitor};
use shakmaty::{CastlingMode, Chess, EnPassantMode, FromSetup, Position, Setup, fen::Fen};

use uci::{
    engine::{Engine, Go},
    search::Search,
};

struct LastPosition {
    pos: Chess,
}

impl LastPosition {
    fn new() -> LastPosition {
        LastPosition {
            pos: Chess::default(),
        }
    }
}

impl Visitor for LastPosition {
    type Result = Chess;

    fn header(&mut self, key: &[u8], value: RawHeader<'_>) {
        // Support games from a non-standard starting position.
        if key == b"FEN" {
            let pos = Fen::from_ascii(value.as_bytes())
                .ok()
                .and_then(|f| f.into_position(CastlingMode::Standard).ok());

            if let Some(pos) = pos {
                self.pos = pos;
            }
        }
    }

    fn begin_variation(&mut self) -> Skip {
        Skip(true) // stay in the mainline
    }

    fn san(&mut self, san_plus: SanPlus) {
        if let Ok(m) = san_plus.san.to_move(&self.pos) {
            self.pos.play_unchecked(&m);
        }
    }

    fn end_game(&mut self) -> Self::Result {
        ::std::mem::replace(&mut self.pos, Chess::default())
    }
}

#[derive(Debug)]
struct Move {
    san: String,
    before: String,
    after: String,
}

#[derive(Default, Debug)]
struct Extractor {
    pos: Chess,
    moves: Vec<Move>,
}

impl Visitor for Extractor {
    type Result = ();

    fn san(&mut self, san_plus: SanPlus) {
        let before = Fen::from(self.pos.clone().into_setup(EnPassantMode::Legal)).to_string();

        if let Ok(m) = san_plus.san.to_move(&self.pos) {
            self.pos.play_unchecked(&m);
        }

        let after = Fen::from(self.pos.clone().into_setup(EnPassantMode::Legal)).to_string();

        self.moves.push(Move {
            san: san_plus.to_string(),
            before,
            after,
        });
    }

    fn end_game(&mut self) -> Self::Result {
        println!("end game");
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let pgn = b"1. f3 e5 2. g4 Qh4#";

    let mut reader = BufferedReader::new_cursor(&pgn[..]);

    let mut visitor = Extractor::default();
    reader.read_game(&mut visitor)?;

    let mut engine = Engine::new("stockfish")?;
    let options = [("Threads", "8"), ("UCI_ShowWDL", "true"), ("MultiPV", "2")];

    engine.uci().await?;

    engine.opts(&options).await?;
    engine.isready().await?;

    // for m in &visitor.moves {
    //     engine
    // }

    println!("{:#?}", visitor);

    Ok(())
}
