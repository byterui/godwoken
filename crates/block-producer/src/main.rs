#[cfg(all(not(target_env = "msvc"), not(target_os = "macos")))]
#[global_allocator]
static GLOBAL_ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use anyhow::{Context, Result};
use clap::{App, Arg, SubCommand};
use gw_block_producer::{db_block_validator, runner, trace};
use gw_config::Config;
use gw_version::Version;
use std::{env, fs, path::Path};

const COMMAND_RUN: &str = "run";
const COMMAND_EXAMPLE_CONFIG: &str = "generate-example-config";
const COMMAND_VERIFY_DB_BLOCK: &str = "verify-db-block";
const ARG_OUTPUT_PATH: &str = "output-path";
const ARG_CONFIG: &str = "config";
const ARG_SKIP_CONFIG_CHECK: &str = "skip-config-check";
const ARG_FROM_BLOCK: &str = "from-block";
const ARG_TO_BLOCK: &str = "to-block";

fn read_config<P: AsRef<Path>>(path: P) -> Result<Config> {
    let content = fs::read(&path)
        .with_context(|| format!("read config file from {}", path.as_ref().to_string_lossy()))?;
    let config = toml::from_slice(&content).with_context(|| "parse config file")?;
    Ok(config)
}

fn generate_example_config<P: AsRef<Path>>(path: P) -> Result<()> {
    let mut config = Config::default();
    config.backends.push(Default::default());
    config.block_producer = Some(Default::default());
    let content = toml::to_string_pretty(&config)?;
    fs::write(path, content)?;
    Ok(())
}

async fn run_cli() -> Result<()> {
    let version = Version::current().to_string();
    let app = App::new("Godwoken")
        .about("The layer2 rollup built upon Nervos CKB.")
        .version(version.as_ref())
        .subcommand(
            SubCommand::with_name(COMMAND_RUN)
                .about("Run Godwoken node")
                .arg(
                    Arg::with_name(ARG_CONFIG)
                        .short("c")
                        .takes_value(true)
                        .required(true)
                        .default_value("./config.toml")
                        .help("The config file path"),
                )
                .arg(
                    Arg::with_name(ARG_SKIP_CONFIG_CHECK)
                        .long(ARG_SKIP_CONFIG_CHECK)
                        .help("Force to accept unsafe config file"),
                )
                .display_order(0),
        )
        .subcommand(
            SubCommand::with_name(COMMAND_EXAMPLE_CONFIG)
                .about("Generate an example config file")
                .arg(
                    Arg::with_name(ARG_OUTPUT_PATH)
                        .short("o")
                        .takes_value(true)
                        .required(true)
                        .default_value("./config.example.toml")
                        .help("The path of the example config file"),
                )
                .display_order(1),
        )
        .subcommand(
            SubCommand::with_name(COMMAND_VERIFY_DB_BLOCK)
                .about("Verify history blocks in db")
                .arg(
                    Arg::with_name(ARG_CONFIG)
                        .short("c")
                        .takes_value(true)
                        .required(true)
                        .default_value("./config.toml")
                        .help("The config file path"),
                )
                .arg(
                    Arg::with_name(ARG_FROM_BLOCK)
                        .short("f")
                        .takes_value(true)
                        .help("From block number"),
                )
                .arg(
                    Arg::with_name(ARG_TO_BLOCK)
                        .short("t")
                        .takes_value(true)
                        .help("To block number"),
                )
                .display_order(2),
        );

    // handle subcommands
    let matches = app.clone().get_matches();
    match matches.subcommand() {
        (COMMAND_RUN, Some(m)) => {
            let config_path = m.value_of(ARG_CONFIG).unwrap();
            let config = read_config(&config_path)?;
            let _guard = trace::init(config.trace)?;
            runner::run(config, m.is_present(ARG_SKIP_CONFIG_CHECK)).await?;
        }
        (COMMAND_EXAMPLE_CONFIG, Some(m)) => {
            let path = m.value_of(ARG_OUTPUT_PATH).unwrap();
            let _guard = trace::init(None)?;
            generate_example_config(path)?;
        }
        (COMMAND_VERIFY_DB_BLOCK, Some(m)) => {
            let config_path = m.value_of(ARG_CONFIG).unwrap();
            let config = read_config(&config_path)?;
            let _guard = trace::init(None)?;
            let from_block: Option<u64> = m.value_of(ARG_FROM_BLOCK).map(str::parse).transpose()?;
            let to_block: Option<u64> = m.value_of(ARG_TO_BLOCK).map(str::parse).transpose()?;
            db_block_validator::verify(config, from_block, to_block).await?;
        }
        _ => {
            // default command: start a Godwoken node
            let config_path = "./config.toml";
            let config = read_config(&config_path)?;
            let _guard = trace::init(config.trace)?;
            runner::run(config, false).await?;
        }
    };
    Ok(())
}

/// Godwoken entry
fn main() -> Result<()> {
    // Supports SMOL_THREADS for backward compatibility.
    let threads = match env::var("SMOL_THREADS").or_else(|_| env::var("GODWOKEN_THREADS")) {
        Err(env::VarError::NotPresent) => num_cpus::get(),
        Err(e) => return Err(e.into()),
        Ok(v) => v.parse()?,
    };
    // - 1 because ChainTask will have a dedicated thread.
    let worker_threads = if threads >= 4 { threads - 1 } else { threads };
    let blocking_threads = match env::var("GODWOKEN_BLOCKING_THREADS") {
        Err(env::VarError::NotPresent) => {
            // set blocking_threads to the number of CPUs because the blocking
            // tasks are CPU bound.
            threads
        }
        Err(e) => return Err(e.into()),
        Ok(v) => v.parse()?,
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .max_blocking_threads(blocking_threads)
        .enable_all()
        .build()?;

    rt.block_on(run_cli())
}
