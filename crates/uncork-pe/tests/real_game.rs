//! Manual check against an installed game:
//!
//! ```sh
//! UNCORK_TEST_GAME_EXE=/path/to/game.exe \
//!     cargo test -p uncork-pe --test real_game -- --ignored --nocapture
//! ```

use std::path::Path;

#[test]
#[ignore = "needs UNCORK_TEST_GAME_EXE pointing at an installed game"]
fn scan_real_game() {
    let Some(exe) = std::env::var_os("UNCORK_TEST_GAME_EXE") else {
        eprintln!("UNCORK_TEST_GAME_EXE is not set; nothing to scan");
        return;
    };
    let scan = uncork_pe::scan_game(Path::new(&exe)).unwrap_or_else(|err| panic!("{err}"));
    println!("{scan:#?}");
    println!("bitness: {:?}", scan.bitness());
    println!("primary API: {:?}", scan.primary_api());
}
