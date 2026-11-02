//! JK2 demo diagnostics use their own decoder and installation root. They never
//! mount JK2 assets in an active JKA session or invoke JKA native prediction.

use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};

pub(crate) fn resolve_demo(root: Option<&Path>, name: &str) -> Result<PathBuf, String> {
    let given = Path::new(name);
    let candidates = if given.is_absolute() {
        vec![given.to_path_buf()]
    } else {
        if given
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err("Use a demo name under fs_jk2path/base/demos, or an absolute path".into());
        }
        let base = root
            .ok_or("Set fs_jk2path to your JK2 GameData directory first")?
            .join("base");
        vec![
            base.join("demos").join(given),
            base.join("jk2demos").join(given),
        ]
    };
    if given.extension().is_some_and(|ext| {
        !ext.to_str().is_some_and(|ext| {
            ext.eq_ignore_ascii_case("dm_15") || ext.eq_ignore_ascii_case("dm_16")
        })
    }) {
        return Err("Expected a JK2 .dm_15 or .dm_16 demo".into());
    }
    let mut matches = Vec::new();
    for path in candidates {
        let paths = if path.extension().is_none() {
            vec![path.with_extension("dm_15"), path.with_extension("dm_16")]
        } else {
            vec![path]
        };
        matches.extend(paths.into_iter().filter(|p| p.is_file()));
    }
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Err(format!(
            "No JK2 demo found for {name}; checked base/demos and base/jk2demos"
        )),
        _ => Err("Multiple JK2 demos match; specify the extension or absolute path".into()),
    }
}

pub(crate) fn inspect(path: &Path) -> Result<String, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    jka_protocol::jk2::demo::inspect(BufReader::new(file))
        .map(|summary| summary.report())
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// A headless entry point in the normal game executable for reproducible user
/// tests. Run before updater/window/GPU/audio startup; print to the caller's
/// stdout so PowerShell redirection and exit codes work normally.
pub(crate) fn cli_entry() -> Option<std::process::ExitCode> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("--jk2-demo-info") {
        return None;
    }
    let result = (|| {
        let name = args
            .next()
            .ok_or("Usage: DinurdoJK --jk2-demo-info <demo name or absolute path>")?;
        if args.next().is_some() {
            return Err("Usage: DinurdoJK --jk2-demo-info <demo name or absolute path>".to_owned());
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let base = exe
            .parent()
            .ok_or("Cannot locate executable directory")?
            .join("base");
        let roots = crate::filesystem_paths::load(&base);
        let path = resolve_demo(roots.jk2path.as_deref(), &name)?;
        inspect(&path)
    })();
    Some(match result {
        Ok(report) => {
            std::println!("{report}");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            std::eprintln!("JK2 demo inspection failed: {error}");
            std::process::ExitCode::FAILURE
        }
    })
}
