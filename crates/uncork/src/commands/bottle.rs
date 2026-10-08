//! `uncork bottle ...`: create, inspect, change and remove bottles.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context as _, anyhow, bail};
use serde::Serialize;
use uncork_core::bottle::{
    self, Bottle, BottleConfig, BottleState, CreateOptions, ImportMode, WindowsVersion,
};
use uncork_core::component::{self, ComponentKind};
use uncork_core::graphics::{self, Backend, BackendChoice};
use uncork_core::launch::{self, LaunchOptions, PlanContext, Target};
use uncork_core::paths::Layout;
use uncork_core::wine::WineRuntime;

use super::{
    Ctx, bottle_wine, bottle_wine_if_installed, confirm_action, is_env_name, key_values,
    newest_wine, shell_quote, yes_no,
};
use crate::cli::BottleCommand;
use crate::output;

/// Dispatch a `bottle` subcommand.
pub(super) fn run(ctx: &Ctx, command: &BottleCommand) -> anyhow::Result<ExitCode> {
    match command {
        BottleCommand::List => list(ctx),
        BottleCommand::Create {
            name,
            wine,
            windows,
        } => create(ctx, name, wine.as_deref(), windows),
        BottleCommand::Info { name } => info(ctx, name),
        BottleCommand::Set { name, settings } => set(ctx, name, settings),
        BottleCommand::Delete { name, yes } => delete(ctx, name, *yes),
        BottleCommand::Import { path, name, move_ } => import(ctx, path, name.as_deref(), *move_),
        BottleCommand::Kill { name } => kill(ctx, name),
        BottleCommand::Env { name } => env(ctx, name),
        BottleCommand::Tool { name, tool } => open_tool(ctx, name, tool),
    }
}

/// One bottle in `bottle list --json`.
#[derive(Debug, Serialize)]
struct BottleSummary<'a> {
    name: &'a str,
    path: &'a Path,
    wine: &'a str,
    wine_installed: bool,
    windows_version: WindowsVersion,
    backend: BackendChoice,
    steam_installed: bool,
    default: bool,
}

fn list(ctx: &Ctx) -> anyhow::Result<ExitCode> {
    let bottles = bottle::list(&ctx.layout)?;
    let default = ctx.config()?.default_bottle;
    let components = ctx.components()?;
    let wine_installed = |bottle: &Bottle| {
        components
            .iter()
            .any(|c| c.meta.kind == ComponentKind::Wine && c.meta.version == bottle.config.wine)
    };
    let summaries: Vec<BottleSummary<'_>> = bottles
        .iter()
        .map(|bottle| BottleSummary {
            name: &bottle.config.name,
            path: &bottle.path,
            wine: &bottle.config.wine,
            wine_installed: wine_installed(bottle),
            windows_version: bottle.config.windows_version,
            backend: bottle.config.graphics.backend,
            steam_installed: uncork_steam::SteamInstall::find(bottle.prefix()).is_some(),
            default: bottle.config.name == default,
        })
        .collect();
    if ctx.json {
        output::print_json(&summaries)?;
        return Ok(ExitCode::SUCCESS);
    }
    if summaries.is_empty() {
        println!("No bottles yet. Run `uncork setup`, or `uncork bottle create <name>`.");
        return Ok(ExitCode::SUCCESS);
    }
    let rows: Vec<Vec<String>> = summaries
        .iter()
        .map(|s| {
            vec![
                if s.default {
                    format!("{} (default)", s.name)
                } else {
                    s.name.to_owned()
                },
                if s.wine_installed {
                    s.wine.to_owned()
                } else {
                    format!("{} (not installed)", s.wine)
                },
                s.windows_version.winecfg_name().to_owned(),
                choice_name(s.backend).to_owned(),
                yes_no(s.steam_installed).to_owned(),
            ]
        })
        .collect();
    print!(
        "{}",
        output::table(&["NAME", "WINE", "WINDOWS", "BACKEND", "STEAM"], &rows)
    );
    Ok(ExitCode::SUCCESS)
}

/// `auto` or the backend name.
fn choice_name(choice: BackendChoice) -> &'static str {
    match choice {
        BackendChoice::Auto => "auto",
        BackendChoice::Fixed(backend) => backend.as_str(),
    }
}

/// Parse `win7`/`win81`/`win10`/`win11` (also `7`, `8.1`, `10`, `11`).
fn parse_windows_version(value: &str) -> anyhow::Result<WindowsVersion> {
    match value.to_ascii_lowercase().as_str() {
        "win7" | "7" => Ok(WindowsVersion::Win7),
        "win81" | "win8.1" | "8.1" | "81" => Ok(WindowsVersion::Win81),
        "win10" | "10" => Ok(WindowsVersion::Win10),
        "win11" | "11" => Ok(WindowsVersion::Win11),
        _ => bail!("unknown Windows version {value:?}; use win7, win81, win10 or win11"),
    }
}

fn create(ctx: &Ctx, name: &str, wine: Option<&str>, windows: &str) -> anyhow::Result<ExitCode> {
    bottle::validate_name(name)?;
    let windows_version = parse_windows_version(windows)?;
    let wine = match wine {
        Some(version) => WineRuntime::from_component(&component::find_installed(
            &ctx.layout,
            ComponentKind::Wine,
            Some(version),
        )?)?,
        None => newest_wine(&ctx.layout)?,
    };
    create_bottle(
        ctx,
        name,
        &wine,
        &CreateOptions {
            windows_version,
            ..CreateOptions::default()
        },
    )?;
    Ok(ExitCode::SUCCESS)
}

/// Create and initialize bottle `name`, telling the user what takes time.
pub(super) fn create_bottle(
    ctx: &Ctx,
    name: &str,
    wine: &WineRuntime,
    options: &CreateOptions,
) -> anyhow::Result<Bottle> {
    eprintln!(
        "Creating bottle {name} with Wine {} (Wine sets up the prefix; this takes about a minute)",
        wine.version
    );
    let created = bottle::create(&ctx.layout, name, wine, options)
        .with_context(|| format!("cannot create bottle {name}"))?;
    println!(
        "Created bottle {name} in {} (Wine {}, {}).",
        created.path.display(),
        wine.version,
        created.config.windows_version.winecfg_name()
    );
    Ok(created)
}

/// Finish creating `bottle` when a failed `create` left it incomplete
/// ([`Bottle::is_initialized`]), with `wine` (the bottle's own); nothing
/// to do otherwise.
pub(super) fn ensure_initialized(
    ctx: &Ctx,
    bottle: &mut Bottle,
    wine: &WineRuntime,
) -> anyhow::Result<()> {
    if bottle.is_initialized() {
        return Ok(());
    }
    let name = bottle.config.name.clone();
    eprintln!(
        "Bottle {name} was not completely created; finishing it with Wine {} (this takes about a minute)",
        wine.version
    );
    bottle::finish_create(&ctx.layout, bottle, wine).with_context(|| {
        format!(
            "cannot finish creating bottle {name}; delete it with `uncork bottle delete {name}` and try again"
        )
    })?;
    println!("Finished creating bottle {name}.");
    Ok(())
}

/// `bottle info --json`.
#[derive(Debug, Serialize)]
struct BottleInfoView<'a> {
    name: &'a str,
    path: &'a Path,
    default: bool,
    initialized: bool,
    wine_installed: bool,
    steam_installed: bool,
    config: &'a BottleConfig,
    state: &'a BottleState,
}

fn info(ctx: &Ctx, name: &str) -> anyhow::Result<ExitCode> {
    let bottle = Bottle::open_named(&ctx.layout, name)?;
    let view = BottleInfoView {
        name: &bottle.config.name,
        path: &bottle.path,
        default: ctx.config()?.default_bottle == bottle.config.name,
        initialized: bottle.is_initialized(),
        wine_installed: component::find_installed(
            &ctx.layout,
            ComponentKind::Wine,
            Some(&bottle.config.wine),
        )
        .is_ok(),
        steam_installed: uncork_steam::SteamInstall::find(bottle.prefix()).is_some(),
        config: &bottle.config,
        state: &bottle.state,
    };
    if ctx.json {
        output::print_json(&view)?;
    } else {
        print!("{}", render_info(&view));
    }
    Ok(ExitCode::SUCCESS)
}

fn render_info(view: &BottleInfoView<'_>) -> String {
    let config = view.config;
    let pinned = |version: &Option<String>| version.as_deref().unwrap_or("newest").to_owned();
    let on_off = |value: bool| if value { "on" } else { "off" };
    let performance = &config.performance;
    let list = |map: &BTreeMap<String, String>| {
        if map.is_empty() {
            "(none)".to_owned()
        } else {
            map.iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    };
    let mut rows = vec![
        (
            "Bottle",
            if view.default {
                format!("{} (default)", view.name)
            } else {
                view.name.to_owned()
            },
        ),
        ("Path", view.path.display().to_string()),
        (
            "Wine",
            if view.wine_installed {
                config.wine.clone()
            } else {
                format!("{} (not installed)", config.wine)
            },
        ),
        ("Windows", config.windows_version.winecfg_name().to_owned()),
        (
            "Initialized",
            match (&view.state.prefix_wine, view.initialized) {
                (Some(version), true) => format!("yes (by Wine {version})"),
                (None, true) => "yes".to_owned(),
                (_, false) => "no".to_owned(),
            },
        ),
        ("Steam", yes_no(view.steam_installed).to_owned()),
        (
            "Graphics",
            format!(
                "backend {}; dxmt {}, dxvk {}, d3dmetal {}",
                choice_name(config.graphics.backend),
                pinned(&config.graphics.dxmt),
                pinned(&config.graphics.dxvk),
                pinned(&config.graphics.d3dmetal)
            ),
        ),
        (
            "Performance",
            format!(
                "msync {}, retina {}, hud {}, metalfx {}, avx {}, game_mode {}",
                on_off(performance.msync),
                on_off(performance.retina),
                on_off(performance.hud),
                on_off(performance.metalfx),
                on_off(performance.avx),
                on_off(performance.game_mode)
            ),
        ),
        ("Environment", list(&config.env)),
        ("DLL overrides", list(&config.dll_overrides)),
    ];
    if let Some(active) = &view.state.active {
        rows.push((
            "Active backend",
            format!(
                "{} {} ({} DLLs in the prefix)",
                active.backend,
                active.version,
                active.dlls.len()
            ),
        ));
    }
    if let Some(source) = &config.imported_from {
        rows.push(("Imported from", source.display().to_string()));
    }
    key_values(&rows)
}

/// Keys `bottle set` understands, for the error message.
const SETTABLE_KEYS: &[&str] = &[
    "wine",
    "windows_version",
    "graphics.backend",
    "graphics.dxmt",
    "graphics.dxvk",
    "graphics.d3dmetal",
    "performance.msync",
    "performance.retina",
    "performance.hud",
    "performance.metalfx",
    "performance.avx",
    "performance.game_mode",
    "env.<NAME>",
    "dll_overrides.<dll>",
];

/// Keys whose values live in the prefix's registry as well (Retina mode
/// with its DPI, the Windows version).
const REGISTRY_KEYS: [&str; 3] = ["performance.retina", "windows_version", "windows"];

fn set(ctx: &Ctx, name: &str, settings: &[String]) -> anyhow::Result<ExitCode> {
    let mut bottle = Bottle::open_named(&ctx.layout, name)?;
    let before = bottle.config.clone();
    let mut changes = Vec::new();
    let mut registry_named = false;
    for setting in settings {
        let (key, value) = setting
            .split_once('=')
            .ok_or_else(|| anyhow!("expected key=value, got {setting:?}"))?;
        let key = key.trim();
        registry_named |= REGISTRY_KEYS.contains(&key);
        let change = apply_setting(&ctx.layout, &mut bottle.config, key, value)
            .with_context(|| format!("cannot set {key}"))?;
        changes.push(change);
    }
    // Naming a registry key also repairs a prefix whose Retina mode and DPI
    // do not match uncork.toml (an imported CrossOver bottle, say), even
    // when uncork.toml stays the same.
    let registry_stale = registry_named
        && bottle.is_initialized()
        && bottle::DisplayRegistry::read(&bottle.path)
            .is_some_and(|registry| registry.differs_from(bottle.config.performance.retina));
    if bottle.config == before && !registry_stale {
        println!("Nothing changed in bottle {name}.");
        return Ok(ExitCode::SUCCESS);
    }

    // The registry goes first: uncork.toml only records a Retina or Windows
    // version change once the prefix has it, so a failed update is retried
    // by running the same command again. A bottle that is not initialized
    // yet gets the registry from its config when it is.
    let registry_changed = registry_stale
        || bottle.config.performance.retina != before.performance.retina
        || bottle.config.windows_version != before.windows_version;
    let registry_updated = registry_changed && bottle.is_initialized();
    let mut stopped = false;
    if registry_updated {
        let wine = bottle_wine(&ctx.layout, &bottle).context(
            "the bottle's registry must change too, and nothing was changed; run this command again once that Wine is installed",
        )?;
        stopped = bottle::apply_registry_stopped(
            &bottle,
            &wine,
            &bottle::default_registry(&bottle.config),
        )
        .context("cannot update the bottle's registry; nothing was changed")?;
    }
    if let Err(err) = bottle.save_config() {
        if registry_updated {
            restore_registry(ctx, &bottle, &before);
        }
        return Err(err.into());
    }
    for change in &changes {
        println!("{name}: {change}");
    }
    if stopped {
        println!(
            "Stopped the Windows programs that were running in bottle {name} (Steam included): Wine reads these settings when a bottle starts. Start them again with `uncork play` or `uncork steam start`."
        );
    }
    if registry_updated {
        let dpi = bottle::dpi_for(bottle.config.performance.retina);
        println!(
            "Updated the bottle's registry (Retina mode {}, {dpi} DPI, Windows version {}).",
            if bottle.config.performance.retina {
                "on"
            } else {
                "off"
            },
            bottle.config.windows_version.winecfg_name()
        );
    }
    if bottle.config.performance.msync != before.performance.msync
        || bottle.config.wine != before.wine
    {
        eprintln!(
            "note: every Wine process of a bottle must share its msync setting and Wine; if anything is running in {name}, stop it with `uncork bottle kill {name}` before the next launch"
        );
    }
    if bottle.config.performance.game_mode && !before.performance.game_mode {
        eprintln!("note: Game Mode launching is experimental (docs/PERFORMANCE.md).");
    }
    Ok(ExitCode::SUCCESS)
}

/// Put the registry of `before` back after its config could not be saved.
fn restore_registry(ctx: &Ctx, bottle: &Bottle, before: &BottleConfig) {
    let previous = Bottle {
        config: before.clone(),
        ..bottle.clone()
    };
    let restored = bottle_wine(&ctx.layout, &previous).and_then(|wine| {
        bottle::apply_registry_stopped(&previous, &wine, &bottle::default_registry(before))
            .map_err(anyhow::Error::from)
    });
    if let Err(err) = restored {
        eprintln!("warning: cannot restore the bottle's previous registry: {err:#}");
    }
}

/// Apply one `key=value` to `config`; returns a description of the change.
fn apply_setting(
    layout: &Layout,
    config: &mut BottleConfig,
    key: &str,
    value: &str,
) -> anyhow::Result<String> {
    let value = value.trim();
    if let Some(name) = key.strip_prefix("env.") {
        if !is_env_name(name) {
            bail!(
                "{name:?} is not a variable name (letters, digits and '_', not starting with a digit)"
            );
        }
        return Ok(if value.is_empty() {
            config.env.remove(name);
            format!("env.{name} removed")
        } else {
            config.env.insert(name.to_owned(), value.to_owned());
            format!("env.{name} = {value}")
        });
    }
    if let Some(dll) = key.strip_prefix("dll_overrides.") {
        let dll = dll.to_ascii_lowercase();
        if dll.is_empty() || dll.contains(['=', ',', ';', ' ', '/', '\\']) {
            bail!("{dll:?} is not a DLL name");
        }
        return Ok(if value == "-" {
            config.dll_overrides.remove(&dll);
            format!("dll_overrides.{dll} removed")
        } else {
            let order = parse_load_order(value)?;
            config.dll_overrides.insert(dll.clone(), order.clone());
            format!("dll_overrides.{dll} = {order:?}")
        });
    }
    match key {
        "wine" => {
            component::validate_version(value)?;
            component::find_installed(layout, ComponentKind::Wine, Some(value))?;
            config.wine = value.to_owned();
        }
        "windows_version" | "windows" => {
            config.windows_version = parse_windows_version(value)?;
            return Ok(format!(
                "windows_version = {}",
                config.windows_version.winecfg_name()
            ));
        }
        "graphics.backend" => {
            config.graphics.backend = parse_backend_choice(value)?;
            return Ok(format!(
                "graphics.backend = {}",
                choice_name(config.graphics.backend)
            ));
        }
        "graphics.dxmt" | "graphics.dxvk" | "graphics.d3dmetal" => {
            let pin = if value.is_empty() {
                None
            } else {
                component::validate_version(value)?;
                Some(value.to_owned())
            };
            let slot = match key {
                "graphics.dxmt" => &mut config.graphics.dxmt,
                "graphics.dxvk" => &mut config.graphics.dxvk,
                _ => &mut config.graphics.d3dmetal,
            };
            *slot = pin;
            return Ok(match slot {
                Some(version) => format!("{key} = {version}"),
                None => format!("{key} unpinned (newest installed)"),
            });
        }
        _ => {
            let Some(switch) = key.strip_prefix("performance.") else {
                return Err(unknown_key(key));
            };
            let on = parse_bool(value)?;
            let performance = &mut config.performance;
            let slot = match switch {
                "msync" => &mut performance.msync,
                "retina" => &mut performance.retina,
                "hud" => &mut performance.hud,
                "metalfx" => &mut performance.metalfx,
                "avx" => &mut performance.avx,
                "game_mode" => &mut performance.game_mode,
                _ => return Err(unknown_key(key)),
            };
            *slot = on;
            return Ok(format!("{key} = {on}"));
        }
    }
    Ok(format!("{key} = {value}"))
}

fn unknown_key(key: &str) -> anyhow::Error {
    anyhow!(
        "unknown setting {key:?}; valid keys: {}",
        SETTABLE_KEYS.join(", ")
    )
}

/// `true`/`false`, `yes`/`no`, `on`/`off`, `1`/`0` (any case).
fn parse_bool(value: &str) -> anyhow::Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => bail!("{value:?} is not a switch value; use true or false (yes/no, on/off, 1/0)"),
    }
}

/// `auto` or a backend name (with the aliases [`Backend`] accepts).
fn parse_backend_choice(value: &str) -> anyhow::Result<BackendChoice> {
    if value.eq_ignore_ascii_case("auto") {
        return Ok(BackendChoice::Auto);
    }
    value
        .parse::<Backend>()
        .map(BackendChoice::Fixed)
        .map_err(|_| {
            anyhow!("unknown backend {value:?}; use auto, dxmt, dxvk, d3dmetal or wined3d")
        })
}

/// A Wine DLL load order: `n`, `b`, `n,b`, `b,n`, `d` or empty (disabled);
/// `native`, `builtin` and `disabled` are accepted and shortened.
fn parse_load_order(value: &str) -> anyhow::Result<String> {
    let invalid = || {
        anyhow!(
            "{value:?} is not a DLL load order; use n, b, n,b, b,n, d, an empty value (disabled), or - to remove the override"
        )
    };
    if value.is_empty() {
        return Ok(String::new());
    }
    let mut parts = Vec::new();
    for part in value.split(',') {
        let short = match part.trim().to_ascii_lowercase().as_str() {
            "n" | "native" => "n",
            "b" | "builtin" => "b",
            "d" | "disabled" => "d",
            _ => return Err(invalid()),
        };
        if parts.contains(&short) {
            return Err(invalid());
        }
        parts.push(short);
    }
    let order = parts.join(",");
    if matches!(order.as_str(), "n" | "b" | "n,b" | "b,n" | "d") {
        Ok(order)
    } else {
        Err(invalid())
    }
}

fn delete(ctx: &Ctx, name: &str, yes: bool) -> anyhow::Result<ExitCode> {
    bottle::validate_name(name)?;
    let path = ctx.layout.bottle_dir(name);
    if !path.is_dir() {
        // Let the core produce its "not found" error with the hint.
        Bottle::open_named(&ctx.layout, name)?;
    }
    confirm_action(
        &format!(
            "Delete bottle {name} ({}) and everything installed in it?",
            path.display()
        ),
        yes,
    )?;
    // A bottle with a broken config can still be deleted; it just has no
    // Wine to stop first.
    let wine = Bottle::open_named(&ctx.layout, name)
        .ok()
        .and_then(|bottle| bottle_wine_if_installed(&ctx.layout, &bottle));
    bottle::delete(&ctx.layout, name, wine.as_ref())
        .with_context(|| format!("cannot delete bottle {name}"))?;
    println!("Deleted bottle {name}.");
    if ctx.config()?.default_bottle == name {
        eprintln!(
            "note: {name} was the default bottle; `uncork setup --bottle {name}` makes a new one"
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// A bottle name from a directory name: lowercase, spaces become `-`,
/// characters bottle names cannot hold are dropped.
fn sanitize_bottle_name(dir_name: &str) -> String {
    let name: String = dir_name
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') => {
                Some(c.to_ascii_lowercase())
            }
            _ => None,
        })
        .collect();
    let name = name.trim_start_matches(['.', '-']);
    name.chars().take(64).collect()
}

fn import(ctx: &Ctx, source: &Path, name: Option<&str>, move_: bool) -> anyhow::Result<ExitCode> {
    let name = match name {
        Some(name) => name.to_owned(),
        None => {
            let resolved = std::fs::canonicalize(source)
                .with_context(|| format!("cannot find {}", source.display()))?;
            let dir_name = resolved
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let name = sanitize_bottle_name(&dir_name);
            if name.is_empty() {
                bail!("cannot make a bottle name from {dir_name:?}; pass --name <name>");
            }
            name
        }
    };
    bottle::validate_name(&name)?;
    let wine = newest_wine(&ctx.layout).context("importing a prefix needs an installed Wine")?;
    let mode = if move_ {
        ImportMode::Move
    } else {
        ImportMode::Clone
    };
    eprintln!("Importing {} as bottle {name}", source.display());
    let imported = bottle::import(&ctx.layout, source, &name, &wine.version, mode)
        .with_context(|| format!("cannot import {}", source.display()))?;
    let how = if move_ {
        "moved"
    } else {
        "cloned; the original is untouched"
    };
    println!(
        "Imported {} as bottle {name} in {} ({how}).",
        source.display(),
        imported.path.display()
    );
    println!("It runs with Wine {}.", wine.version);
    if let Some(user) = imported.config.env.get("USER") {
        println!(
            "The prefix's Windows user is {user:?}, so the bottle runs Wine with USER={user} and LOGNAME={user} to keep its AppData (sign-ins, game settings)."
        );
    }
    apply_imported_display(&imported, &wine);
    Ok(ExitCode::SUCCESS)
}

/// Make an imported bottle's Retina mode and DPI a consistent pair from
/// the first launch: `bottle::import` kept the prefix's choice in
/// `performance.retina`; write the matching registry now. A failure is a
/// warning naming the command that retries it.
fn apply_imported_display(imported: &Bottle, wine: &WineRuntime) {
    let name = &imported.config.name;
    let retina = imported.config.performance.retina;
    let found = bottle::DisplayRegistry::read(&imported.path).unwrap_or_default();
    let dpi = bottle::dpi_for(retina);
    eprintln!(
        "Writing Retina mode and DPI into the bottle's registry (Wine may update the prefix first; that can take a minute)"
    );
    let on_off = if retina { "on" } else { "off" };
    match bottle::apply_registry_stopped(
        imported,
        wine,
        &bottle::default_registry(&imported.config),
    ) {
        Ok(_) if found == bottle::DisplayRegistry::default() => println!(
            "The prefix set neither Retina mode nor its DPI, so the bottle has Uncork's default: Retina mode off with {dpi} DPI; change it with `uncork bottle set {name} performance.retina=true`."
        ),
        Ok(_) => println!(
            "The prefix had {}, so Retina mode stays {on_off} (performance.retina = {retina}) with {dpi} DPI to match; change it with `uncork bottle set {name} performance.retina={}`.",
            found.describe(),
            !retina
        ),
        Err(err) => eprintln!(
            "warning: cannot write Retina mode and DPI into bottle {name}'s registry: {err}; run `uncork bottle set {name} performance.retina={retina}` to try again"
        ),
    }
}

fn kill(ctx: &Ctx, name: &str) -> anyhow::Result<ExitCode> {
    let bottle = Bottle::open_named(&ctx.layout, name)?;
    let wine = bottle_wine(&ctx.layout, &bottle)?;
    let stopped = match uncork_core::process::run(&launch::in_bottle(
        wine.kill_command(bottle.prefix()),
        &bottle,
        &wine,
    )) {
        Ok(()) => true,
        // wineserver exits non-zero when there was nothing to stop.
        Err(uncork_core::Error::Command { status, .. }) if status.starts_with("exited") => false,
        Err(err) => return Err(err).context(format!("cannot stop bottle {name}")),
    };
    // Nothing runs in the bottle now. A Steam client that was killed, here
    // or earlier (a crash, a force quit), left its pid in the registry,
    // which would make `uncork play` think Steam is still running.
    if uncork_steam::SteamInstall::find(bottle.prefix()).is_some()
        && let Err(err) = uncork_core::steam::forget_client(&bottle, &wine)
    {
        tracing::warn!("cannot reset Steam's running marker: {err}");
    }
    if stopped {
        println!("Stopped every Windows process in bottle {name}.");
    } else {
        println!("Nothing was running in bottle {name}.");
    }
    Ok(ExitCode::SUCCESS)
}

fn env(ctx: &Ctx, name: &str) -> anyhow::Result<ExitCode> {
    let bottle = Bottle::open_named(&ctx.layout, name)?;
    let wine = bottle_wine(&ctx.layout, &bottle)?;
    print!(
        "{}",
        render_env(&bottle, &wine, launch::base_env(&bottle, &wine, None))
    );
    Ok(ExitCode::SUCCESS)
}

/// `export` lines for using `bottle` by hand: the base environment Uncork
/// gives Wine (minus pass-through variables the shell already has), the
/// bottle's own `env`, `WINE`/`WINESERVER`, and Wine's `bin` on `PATH`.
fn render_env(bottle: &Bottle, wine: &WineRuntime, base: BTreeMap<String, String>) -> String {
    let mut vars = base;
    vars.remove("PATH");
    for name in launch::ALLOWED_ENV {
        if vars.get(*name).map(String::as_str) == std::env::var(name).ok().as_deref() {
            vars.remove(*name);
        }
    }
    vars.extend(bottle.config.env.clone());
    vars.insert(
        "WINEPREFIX".to_owned(),
        bottle.prefix().to_string_lossy().into_owned(),
    );
    vars.insert(
        "WINE".to_owned(),
        wine.wine_bin().to_string_lossy().into_owned(),
    );
    vars.insert(
        "WINESERVER".to_owned(),
        wine.wineserver_bin().to_string_lossy().into_owned(),
    );
    if !vars.contains_key("WINEDLLOVERRIDES") {
        let mut overrides = BTreeMap::from([("winemenubuilder.exe".to_owned(), "d".to_owned())]);
        overrides.extend(bottle.config.dll_overrides.clone());
        vars.insert(
            "WINEDLLOVERRIDES".to_owned(),
            graphics::render_overrides(&overrides),
        );
    }
    let name = &bottle.config.name;
    let mut out = format!(
        "# Bottle {name} (Wine {}). Use it with: eval \"$(uncork bottle env {name})\"\n",
        wine.version
    );
    for (key, value) in &vars {
        if is_env_name(key) {
            out.push_str(&format!("export {key}={}\n", shell_quote(value)));
        } else {
            out.push_str(&format!("# skipped {key:?}: not a shell variable name\n"));
        }
    }
    let bin = wine.root.join("bin");
    out.push_str(&format!(
        "export PATH={}:\"$PATH\"\n",
        shell_quote(&bin.to_string_lossy())
    ));
    out
}

/// Wine tools `bottle tool` opens.
const TOOLS: [&str; 6] = [
    "winecfg", "regedit", "taskmgr", "explorer", "cmd", "control",
];

fn open_tool(ctx: &Ctx, name: &str, tool: &str) -> anyhow::Result<ExitCode> {
    let tool = tool.to_ascii_lowercase();
    let tool = tool.strip_suffix(".exe").unwrap_or(&tool);
    if !TOOLS.contains(&tool) {
        bail!("unknown tool {tool:?}; choose one of {}", TOOLS.join(", "));
    }
    let mut bottle = Bottle::open_named(&ctx.layout, name)?;
    let wine = bottle_wine(&ctx.layout, &bottle)?;
    let components = ctx.components()?;
    // `cmd` needs a console window of its own; Uncork's own stdin is not one.
    let target = if tool == "cmd" {
        Target::WineProgram {
            name: "wineconsole".to_owned(),
            args: vec!["cmd".to_owned()],
        }
    } else {
        Target::WineProgram {
            name: tool.to_owned(),
            args: Vec::new(),
        }
    };
    let plan = launch::plan(
        PlanContext {
            layout: &ctx.layout,
            bottle: &bottle,
            wine: &wine,
            components: &components,
            profile: None,
        },
        &target,
        &LaunchOptions::default(),
    )?;
    super::launch::print_warnings(&plan.warnings);
    let child = launch::execute(&plan, &mut bottle, &wine)
        .with_context(|| format!("cannot start {tool}"))?;
    println!("Started {tool} in bottle {name} (pid {}).", child.id());
    println!("Log: {}", plan.log.display());
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use uncork_core::bottle::{GraphicsConfig, PerformanceConfig};

    use super::*;

    fn config() -> BottleConfig {
        BottleConfig {
            schema: 1,
            name: "steam".to_owned(),
            wine: "11.0".to_owned(),
            windows_version: WindowsVersion::Win10,
            graphics: GraphicsConfig::default(),
            performance: PerformanceConfig::default(),
            env: BTreeMap::new(),
            dll_overrides: BTreeMap::new(),
            imported_from: None,
        }
    }

    fn set(config: &mut BottleConfig, key: &str, value: &str) -> anyhow::Result<String> {
        apply_setting(&Layout::at("/nonexistent"), config, key, value)
    }

    #[test]
    fn switches_and_backends() {
        let mut config = config();
        set(&mut config, "performance.hud", "on").unwrap();
        set(&mut config, "performance.msync", "0").unwrap();
        set(&mut config, "performance.game_mode", "YES").unwrap();
        assert!(config.performance.hud && config.performance.game_mode);
        assert!(!config.performance.msync);
        assert_eq!(
            set(&mut config, "graphics.backend", "DXMT").unwrap(),
            "graphics.backend = dxmt"
        );
        assert_eq!(config.graphics.backend, BackendChoice::Fixed(Backend::Dxmt));
        set(&mut config, "graphics.backend", "auto").unwrap();
        assert_eq!(config.graphics.backend, BackendChoice::Auto);
        assert!(set(&mut config, "graphics.backend", "metal").is_err());
        assert!(set(&mut config, "performance.hud", "maybe").is_err());
    }

    #[test]
    fn version_pins_and_windows() {
        let mut config = config();
        set(&mut config, "graphics.dxmt", "0.80").unwrap();
        assert_eq!(config.graphics.dxmt.as_deref(), Some("0.80"));
        assert_eq!(
            set(&mut config, "graphics.dxmt", "").unwrap(),
            "graphics.dxmt unpinned (newest installed)"
        );
        assert_eq!(config.graphics.dxmt, None);
        assert!(set(&mut config, "graphics.dxvk", "../x").is_err());
        set(&mut config, "windows_version", "win7").unwrap();
        assert_eq!(config.windows_version, WindowsVersion::Win7);
        assert!(set(&mut config, "windows_version", "xp").is_err());
    }

    #[test]
    fn wine_must_be_installed() {
        let mut config = config();
        let err = set(&mut config, "wine", "12.0").unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
        assert_eq!(config.wine, "11.0");
    }

    #[test]
    fn env_and_overrides() {
        let mut config = config();
        set(&mut config, "env.DXMT_LOG_LEVEL", "info").unwrap();
        assert_eq!(config.env["DXMT_LOG_LEVEL"], "info");
        set(&mut config, "env.DXMT_LOG_LEVEL", "").unwrap();
        assert!(config.env.is_empty());
        assert!(set(&mut config, "env.1BAD", "x").is_err());

        set(
            &mut config,
            "dll_overrides.D3DCompiler_47",
            "native,builtin",
        )
        .unwrap();
        assert_eq!(config.dll_overrides["d3dcompiler_47"], "n,b");
        set(&mut config, "dll_overrides.xinput1_3", "").unwrap();
        assert_eq!(config.dll_overrides["xinput1_3"], "");
        set(&mut config, "dll_overrides.d3dcompiler_47", "-").unwrap();
        assert!(!config.dll_overrides.contains_key("d3dcompiler_47"));
        assert!(set(&mut config, "dll_overrides.x", "n,n").is_err());
        assert!(set(&mut config, "dll_overrides.x", "fast").is_err());
        assert!(set(&mut config, "dll_overrides.a=b", "n").is_err());
    }

    #[test]
    fn unknown_keys_list_the_valid_ones() {
        let mut config = config();
        let err = set(&mut config, "performance.turbo", "1").unwrap_err();
        assert!(err.to_string().contains("performance.retina"), "{err}");
        let err = set(&mut config, "colour", "red").unwrap_err();
        assert!(err.to_string().contains("valid keys: wine"), "{err}");
    }

    #[test]
    fn load_orders() {
        for (input, expected) in [
            ("n", "n"),
            ("B", "b"),
            ("n,b", "n,b"),
            ("builtin,native", "b,n"),
            ("disabled", "d"),
            ("", ""),
        ] {
            assert_eq!(parse_load_order(input).unwrap(), expected, "{input}");
        }
        for input in ["x", "n,d", "b,b", "n,b,d"] {
            assert!(parse_load_order(input).is_err(), "{input}");
        }
    }

    #[test]
    fn import_names() {
        assert_eq!(sanitize_bottle_name("Steam"), "steam");
        assert_eq!(sanitize_bottle_name("My Games (2024)"), "my-games-2024");
        assert_eq!(sanitize_bottle_name(".hidden"), "hidden");
        assert_eq!(sanitize_bottle_name("Ünïcode"), "ncode");
        assert_eq!(sanitize_bottle_name("!!!"), "");
        assert_eq!(sanitize_bottle_name(&"a".repeat(80)).len(), 64);
    }

    #[test]
    fn windows_versions() {
        assert_eq!(
            parse_windows_version("Win10").unwrap(),
            WindowsVersion::Win10
        );
        assert_eq!(parse_windows_version("8.1").unwrap(), WindowsVersion::Win81);
        assert!(parse_windows_version("vista").is_err());
    }

    #[test]
    fn env_lines_quote_values_and_add_wine() {
        let mut bottle_config = config();
        bottle_config
            .env
            .insert("USER".to_owned(), "crossover".to_owned());
        bottle_config
            .dll_overrides
            .insert("d3dcompiler_47".to_owned(), "n,b".to_owned());
        let bottle = Bottle {
            path: "/data/bottles/steam".into(),
            config: bottle_config,
            state: BottleState::default(),
        };
        let wine = WineRuntime {
            root: "/data/components/wine/11.0".into(),
            version: "11.0".to_owned(),
            features: Vec::new(),
        };
        let base = BTreeMap::from([
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("WINEDEBUG".to_owned(), "-all".to_owned()),
            ("WINEMSYNC".to_owned(), "1".to_owned()),
            ("A B".to_owned(), "x".to_owned()),
        ]);
        let text = render_env(&bottle, &wine, base);
        assert!(text.starts_with("# Bottle steam (Wine 11.0)."), "{text}");
        assert!(
            text.contains("export WINEPREFIX='/data/bottles/steam'\n"),
            "{text}"
        );
        assert!(text.contains("export WINEMSYNC='1'\n"), "{text}");
        assert!(text.contains("export USER='crossover'\n"), "{text}");
        assert!(
            text.contains("export WINEDLLOVERRIDES='winemenubuilder.exe=d;d3dcompiler_47=n,b'\n"),
            "{text}"
        );
        assert!(
            text.contains("export WINE='/data/components/wine/11.0/bin/wine64'\n"),
            "{text}"
        );
        assert!(text.contains("# skipped \"A B\""), "{text}");
        assert!(
            text.ends_with("export PATH='/data/components/wine/11.0/bin':\"$PATH\"\n"),
            "{text}"
        );
        assert!(!text.contains("/usr/bin:/bin"), "{text}");
    }
}
