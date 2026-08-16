use anyhow::{Result, anyhow};
use clap::ValueEnum;

use ff13::{Game, GameInstall, discovery};

#[derive(Copy, Clone, ValueEnum)]
pub enum GameArg {
    Xiii,
    Xiii2,
    Lr,
}

impl From<GameArg> for Game {
    fn from(g: GameArg) -> Self {
        match g {
            GameArg::Xiii => Game::XIII,
            GameArg::Xiii2 => Game::XIII2,
            GameArg::Lr => Game::LR,
        }
    }
}

pub fn game_name_of(game: Game) -> &'static str {
    match game {
        Game::XIII => "xiii",
        Game::XIII2 => "xiii2",
        Game::LR => "lr",
    }
}

pub fn resolve(game: Game) -> Result<GameInstall> {
    let found = discovery::installs(game);
    match found.len() {
        0 => Err(anyhow!(
            "could not locate {}; add it with `install {} --path <dir>` if it isn't a Steam copy",
            game.display_name(),
            game_name_of(game),
        )),
        1 => Ok(found.into_iter().next().unwrap()),
        _ => choose_install(found),
    }
}

pub fn choose_install(installs: Vec<GameInstall>) -> Result<GameInstall> {
    use std::io::Write;
    println!("Multiple installs found:");
    for (i, gi) in installs.iter().enumerate() {
        println!(
            "  {}) {} ({})",
            i + 1,
            gi.game.display_name(),
            gi.root.display()
        );
    }
    print!("Choose [1-{}]: ", installs.len());
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    match line.trim().parse::<usize>() {
        Ok(n) if (1..=installs.len()).contains(&n) => Ok(installs.into_iter().nth(n - 1).unwrap()),
        _ => anyhow::bail!("not a valid choice"),
    }
}
