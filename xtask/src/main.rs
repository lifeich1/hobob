use anyhow::Result;
use duct::cmd;
use lintd_taskops::ops::Recipe;
use lintd_taskops::{Addon, Make};

struct Maker();
impl Addon for Maker {
    fn dist() -> Result<()> {
        cmd!("cross", "-v", "build", "--bin", "hobob", "-r").go()?;
        cmd!(
            "scp",
            "./target/armv7-unknown-linux-gnueabihf/release/hobob",
            "lclpi:/lintd/"
        )
        .go()?;
        Ok(())
    }
}

fn main() -> Result<()> {
    Maker::make()
}
