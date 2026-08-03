use std::io::{self, Write};

use anyhow::{Result, bail};
use futures::StreamExt;
use microsandbox::{
    Sandbox,
    logs::{LogOptions, LogSource, LogStreamOptions, LogStreamStart},
};

use crate::app::App;
use crate::cli::LogsArgs;

pub(crate) fn log_sources(value: Option<&str>) -> Result<Vec<LogSource>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let mut sources = Vec::new();
    for source in value.split(',') {
        let selected = match source.trim() {
            "stdout" => vec![LogSource::Stdout],
            "stderr" => vec![LogSource::Stderr],
            "output" => vec![LogSource::Output],
            "system" => vec![LogSource::System],
            "all" => vec![
                LogSource::Stdout,
                LogSource::Stderr,
                LogSource::Output,
                LogSource::System,
            ],
            other => {
                bail!("invalid log source '{other}' (use stdout, stderr, output, system, or all)")
            }
        };
        for source in selected {
            if !sources.contains(&source) {
                sources.push(source);
            }
        }
    }
    Ok(sources)
}

pub(crate) fn print_log(entry: &microsandbox::logs::LogEntry) -> Result<()> {
    io::stdout().write_all(&entry.data)?;
    io::stdout().flush()?;
    Ok(())
}

pub(crate) async fn cmd_logs(app: &App, args: LogsArgs) -> Result<()> {
    app.require_row(&args.name)?;
    let sources = log_sources(args.source.as_deref())?;
    let options = LogOptions {
        tail: args.tail,
        sources: sources.clone(),
        ..Default::default()
    };
    let snapshot = microsandbox::logs::read_logs_snapshot(&args.name, &options).await?;
    for entry in &snapshot.entries {
        print_log(entry)?;
    }
    if args.follow {
        let handle = Sandbox::get(&args.name).await?;
        let mut stream = handle
            .log_stream(&LogStreamOptions {
                sources,
                start: LogStreamStart::From(snapshot.cursor),
                until: None,
                follow: true,
            })
            .await?;
        while let Some(entry) = stream.next().await {
            print_log(&entry?)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_log_source_sets() {
        assert_eq!(log_sources(Some("stdout,stderr")).unwrap().len(), 2);
        assert_eq!(log_sources(Some("all")).unwrap().len(), 4);
        assert!(log_sources(Some("unknown")).is_err());
    }
}
