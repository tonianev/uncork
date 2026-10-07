//! Command-line interface definition (clap derive). Handlers live in
//! `commands/`; this file only describes the surface. `docs/CLI.md` is
//! generated from the same text, so keep help strings short and exact.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Run Windows games on Apple Silicon Macs.
///
/// Uncork sets up Wine, a Metal translation layer (DXMT, DXVK or Apple's
/// D3DMetal) and the Windows Steam client in a "bottle", then launches games
/// with per-game settings tuned for speed.
///
/// First time? Run `uncork setup`, then `uncork play rise-of-nations`.
#[derive(Debug, Parser)]
#[command(name = "uncork", version, propagate_version = true, max_term_width = 100)]
pub struct Cli {
    /// Print machine-readable JSON instead of text (list, info and inspect commands).
    #[arg(long, global = true)]
    pub json: bool,

    /// More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// The command.
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Check this Mac and the Uncork installation, and say how to fix problems.
    Doctor,

    /// One-shot setup: install the recommended components, create the
    /// `steam` bottle and install Steam into it.
    Setup(SetupArgs),

    /// Find a game profile and play it (through Steam when the game is on Steam).
    Play(PlayArgs),

    /// Run a Windows program in a bottle.
    Run(RunArgs),

    /// Show what a Windows executable is: 32/64-bit, graphics API, recommended backend.
    Inspect(InspectArgs),

    /// Manage Wine and graphics components.
    #[command(subcommand)]
    Runtime(RuntimeCommand),

    /// Manage bottles (Wine prefixes).
    #[command(subcommand)]
    Bottle(BottleCommand),

    /// Install and use the Windows Steam client in a bottle.
    #[command(subcommand)]
    Steam(SteamCommand),

    /// List and show game profiles.
    #[command(subcommand)]
    Profile(ProfileCommand),

    /// Run winetricks verbs in a bottle (needs `brew install winetricks`).
    Winetricks(WinetricksArgs),
}

/// Graphics backend selection on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BackendArg {
    /// Pick per game from its API and bitness.
    Auto,
    /// Apple D3DMetal (64-bit D3D11/D3D12; import with `uncork runtime import-gptk`).
    D3dmetal,
    /// DXMT (D3D10/11 to Metal, 32- and 64-bit).
    Dxmt,
    /// DXVK on MoltenVK (D3D9/10/11).
    Dxvk,
    /// Wine's built-in WineD3D (OpenGL), the compatibility fallback.
    Wined3d,
}

/// Options shared by commands that launch something.
#[derive(Debug, Clone, Args)]
pub struct LaunchFlags {
    /// Bottle to use (default: `default_bottle` from config, normally `steam`).
    #[arg(short, long)]
    pub bottle: Option<String>,

    /// Graphics backend for this launch.
    #[arg(long, value_enum)]
    pub backend: Option<BackendArg>,

    /// Show Apple's Metal performance HUD (FPS, frame time, GPU).
    #[arg(long)]
    pub hud: bool,

    /// Upscale with MetalFX where the backend supports it.
    #[arg(long)]
    pub metalfx: bool,

    /// Render at native Retina resolution.
    #[arg(long)]
    pub retina: bool,

    /// Enable Wine debug channels (e.g. `+loaddll,+d3d`); output goes to the launch log.
    #[arg(long, value_name = "CHANNELS")]
    pub wine_debug: Option<String>,

    /// Extra environment variable for this launch (repeatable).
    #[arg(short, long = "env", value_name = "KEY=VALUE")]
    pub env: Vec<String>,

    /// Print the launch plan (command, environment, DLLs to install) and exit.
    #[arg(long)]
    pub dry_run: bool,

    /// Wait for the program and its wineserver to exit before returning.
    #[arg(long)]
    pub wait: bool,
}

/// `uncork setup`.
#[derive(Debug, Args)]
pub struct SetupArgs {
    /// Name of the bottle to create for Steam.
    #[arg(long, default_value = "steam")]
    pub bottle: String,

    /// Do not install Steam (components and bottle only).
    #[arg(long)]
    pub no_steam: bool,

    /// Answer yes to downloads.
    #[arg(short, long)]
    pub yes: bool,
}

/// `uncork play`.
#[derive(Debug, Args)]
pub struct PlayArgs {
    /// Profile id, Steam app id, or part of a game's name.
    pub game: String,

    /// Shared launch options.
    #[command(flatten)]
    pub launch: LaunchFlags,

    /// Arguments passed to the game after the profile's own.
    #[arg(last = true)]
    pub args: Vec<String>,
}

/// `uncork run`.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// Windows executable: a macOS path, or a Windows path (`C:\...`) inside the bottle.
    pub exe: String,

    /// Shared launch options.
    #[command(flatten)]
    pub launch: LaunchFlags,

    /// Arguments passed to the program.
    #[arg(last = true)]
    pub args: Vec<String>,
}

/// `uncork inspect`.
#[derive(Debug, Args)]
pub struct InspectArgs {
    /// Path to a Windows `.exe`.
    pub exe: PathBuf,
}

/// `uncork runtime ...`.
#[derive(Debug, Subcommand)]
pub enum RuntimeCommand {
    /// List installed components.
    List,
    /// List components available to install (the pinned catalog).
    Available,
    /// Install components: `wine`, `dxmt`, `dxvk`, or `all` (default: all recommended).
    Install {
        /// Component kinds.
        kinds: Vec<String>,
        /// Exact version (only with a single kind).
        #[arg(long)]
        version: Option<String>,
        /// Answer yes to downloads.
        #[arg(short, long)]
        yes: bool,
    },
    /// Remove an installed component version.
    Remove {
        /// Component kind.
        kind: String,
        /// Version.
        version: String,
    },
    /// Import D3DMetal from your copy of Apple's Game Porting Toolkit (mounted .dmg or its redist folder).
    ImportGptk {
        /// Path to the mounted GPTK volume, its `redist`, or `redist/lib`.
        path: PathBuf,
    },
}

/// `uncork bottle ...`.
#[derive(Debug, Subcommand)]
pub enum BottleCommand {
    /// List bottles.
    List,
    /// Create a bottle.
    Create {
        /// Bottle name.
        name: String,
        /// Wine component version (default: newest installed).
        #[arg(long)]
        wine: Option<String>,
        /// Windows version: win7, win81, win10, win11.
        #[arg(long, default_value = "win10")]
        windows: String,
    },
    /// Show a bottle's settings and state.
    Info {
        /// Bottle name.
        name: String,
    },
    /// Change a bottle setting, e.g. `graphics.backend=dxmt`, `performance.retina=true`, `env.DXMT_LOG_LEVEL=info`.
    Set {
        /// Bottle name.
        name: String,
        /// `key=value` pairs.
        #[arg(required = true)]
        settings: Vec<String>,
    },
    /// Delete a bottle and everything installed in it.
    Delete {
        /// Bottle name.
        name: String,
        /// Do not ask for confirmation.
        #[arg(short, long)]
        yes: bool,
    },
    /// Import an existing Wine prefix (CrossOver or Whisky bottle, plain WINEPREFIX).
    Import {
        /// The prefix directory (contains `drive_c` and `system.reg`).
        path: PathBuf,
        /// Name for the new bottle (default: the directory name).
        #[arg(long)]
        name: Option<String>,
        /// Move instead of cloning (cloning is instant on APFS and leaves the original untouched).
        #[arg(long = "move")]
        move_: bool,
    },
    /// Stop every Windows process in a bottle.
    Kill {
        /// Bottle name.
        name: String,
    },
    /// Print `export` lines to use a bottle with Wine tools by hand.
    Env {
        /// Bottle name.
        name: String,
    },
    /// Open a Wine tool: winecfg, regedit, taskmgr, explorer, cmd, control.
    Tool {
        /// Bottle name.
        name: String,
        /// Tool name.
        tool: String,
    },
}

/// `uncork steam ...`.
#[derive(Debug, Subcommand)]
pub enum SteamCommand {
    /// Download Valve's installer and install Steam into a bottle.
    Install {
        /// Bottle (default: `steam`).
        #[arg(short, long)]
        bottle: Option<String>,
        /// Answer yes to the download.
        #[arg(short, long)]
        yes: bool,
    },
    /// Start the Steam client.
    Start {
        /// Shared launch options.
        #[command(flatten)]
        launch: LaunchFlags,
    },
    /// List games installed in a bottle's Steam libraries.
    Games {
        /// Bottle (default: `steam`).
        #[arg(short, long)]
        bottle: Option<String>,
    },
    /// Launch a Steam game by app id.
    Launch {
        /// Steam app id.
        appid: u32,
        /// Shared launch options.
        #[command(flatten)]
        launch: LaunchFlags,
        /// Arguments passed to the game.
        #[arg(last = true)]
        args: Vec<String>,
    },
}

/// `uncork profile ...`.
#[derive(Debug, Subcommand)]
pub enum ProfileCommand {
    /// List profiles with their compatibility status.
    List,
    /// Show one profile.
    Show {
        /// Profile id, Steam app id or part of the name.
        game: String,
    },
}

/// `uncork winetricks`.
#[derive(Debug, Args)]
pub struct WinetricksArgs {
    /// Bottle (default: `steam`).
    #[arg(short, long)]
    pub bottle: Option<String>,
    /// Verbs, e.g. `corefonts vcrun2022`.
    #[arg(required = true)]
    pub verbs: Vec<String>,
}
