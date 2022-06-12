//! Doc command
use crate::{
    cmd::{
        forge::{build::CoreBuildArgs, watch::WatchArgs},
        Cmd,
    },
    compile,
    compile::ProjectCompiler,
};
use askama::Template;
use clap::{AppSettings, Parser};
use ethers::solc::artifacts::{output_selection::OutputSelection, DevDoc, UserDoc};
use forge::executor::opts::EvmOpts;
use foundry_common::evm::EvmArgs;
use foundry_config::{figment::Figment, Config};
use globset::Glob;
use regex::Regex;
use std::collections::BTreeMap;
use std::{
    fmt::Debug,
    fs,
    path::{Path, PathBuf},
};
// use tera::{Context, Tera};
use watchexec::config::{InitConfig, RuntimeConfig};

// Loads project's figment and merges the build cli arguments into it
foundry_config::impl_figment_convert!(DocArgs, opts, evm_opts);

#[derive(Debug, Clone, Parser)]
#[clap(global_setting = AppSettings::DeriveDisplayOrder)]
pub struct DocArgs {
    /// Run a test in the debugger.
    ///
    /// The argument passed to this flag is the name of the test function you want to run, and it
    /// works the same as --match-test.
    ///
    /// If more than one test matches your specified criteria, you must add additional filters
    /// until only one test is found (see --match-contract and --match-path).
    ///
    /// The matching test will be opened in the debugger regardless of the outcome of the test.
    ///
    /// If the matching test is a fuzz test, then it will open the debugger on the first failure
    /// case.
    /// If the fuzz test does not fail, it will open the debugger on the last fuzz case.
    ///
    /// For more fine-grained control of which fuzz case is run, see forge run.
    #[clap(long, value_name = "TEST_FUNCTION")]
    debug: Option<Regex>,

    /// Print a gas report.
    #[clap(long, env = "FORGE_GAS_REPORT")]
    gas_report: bool,

    /// Exit with code 0 even if a test fails.
    #[clap(long, env = "FORGE_ALLOW_FAILURE")]
    allow_failure: bool,

    /// Output test results in JSON format.
    #[clap(long, short, help_heading = "DISPLAY OPTIONS")]
    json: bool,

    #[clap(flatten, next_help_heading = "EVM OPTIONS")]
    evm_opts: EvmArgs,

    #[clap(
        long,
        env = "ETHERSCAN_API_KEY",
        help = "Set etherscan api key to better decode traces",
        value_name = "ETHERSCAN_KEY"
    )]
    etherscan_api_key: Option<String>,

    #[clap(flatten, next_help_heading = "BUILD OPTIONS")]
    opts: CoreBuildArgs,

    #[clap(flatten, next_help_heading = "WATCH OPTIONS")]
    pub watch: WatchArgs,

    /// List tests instead of running them
    #[clap(long, short, help_heading = "DISPLAY OPTIONS")]
    list: bool,
}

impl DocArgs {
    /// Returns the flattened [`CoreBuildArgs`]
    pub fn build_args(&self) -> &CoreBuildArgs {
        &self.opts
    }

    /// Returns the currently configured [Config] and the extracted [EvmOpts] from that config
    pub fn config_and_evm_opts(&self) -> eyre::Result<(Config, EvmOpts)> {
        // merge all configs
        let figment: Figment = self.into();
        let evm_opts = figment.extract()?;
        let mut config = Config::from_provider(figment).sanitized();

        // merging etherscan api key into Config
        if let Some(etherscan_api_key) = &self.etherscan_api_key {
            config.etherscan_api_key = Some(etherscan_api_key.to_string());
        }
        Ok((config, evm_opts))
    }

    /// Returns whether `BuildArgs` was configured with `--watch`
    pub fn is_watch(&self) -> bool {
        self.watch.watch.is_some()
    }

    /// Returns the [`watchexec::InitConfig`] and [`watchexec::RuntimeConfig`] necessary to
    /// bootstrap a new [`watchexe::Watchexec`] loop.
    pub(crate) fn watchexec_config(&self) -> eyre::Result<(InitConfig, RuntimeConfig)> {
        self.watch.watchexec_config(|| {
            let config = Config::from(self);
            vec![config.src, config.test]
        })
    }
}

#[derive(Template, Debug)] // this will generate the code...
#[template(path = "doc.md")]
struct Document {
    file: String,
    contracts: Vec<Contract>,
}

#[derive(Debug)] // this will generate the code...
struct Contract {
    name: String,
    userdoc: UserDoc,
    devdoc: DevDoc,
}

impl Cmd for DocArgs {
    type Output = ();

    fn run(self) -> eyre::Result<Self::Output> {
        // Merge all configs
        let (config, mut evm_opts) = self.config_and_evm_opts()?;

        // Set up the project
        let mut project = config.project()?;
        // TODO: better way to set this up?
        project.solc_config.settings.output_selection = OutputSelection(BTreeMap::from([(
            "*".to_string(),
            BTreeMap::from([("*".to_string(), vec!["devdoc".to_string(), "userdoc".to_string()])]),
        )]));
        let compiler = ProjectCompiler::default();
        let output = if self.opts.silent {
            compile::suppress_compile(&project)
        } else {
            compiler.compile(&project)
        }?;

        let src_dir = config.src.to_str().unwrap();
        let src_dir_glob = Glob::new(format!("{}/*", src_dir).as_str())?;
        let documents: Vec<Document> = output
            .output()
            .contracts
            .0
            .iter()
            .filter(|(file, _)| src_dir_glob.compile_matcher().is_match(file))
            .map(|(file, contracts)| Document {
                file: file
                    .to_string()
                    .strip_prefix(format!("{}/", src_dir).as_str())
                    .unwrap()
                    .strip_suffix(".sol")
                    .unwrap()
                    .to_string(),
                contracts: contracts
                    .iter()
                    .map(|(name, contract)| Contract {
                        name: name.to_string(),
                        userdoc: contract[0].clone().contract.userdoc,
                        devdoc: contract[0].clone().contract.devdoc,
                    })
                    .collect(),
            })
            .collect();

        let mut doc_dir = PathBuf::new();
        doc_dir.push(src_dir);
        doc_dir.pop();
        doc_dir.push("docs");
        let doc_dir = doc_dir.as_path();
        if !doc_dir.exists() {
            fs::create_dir(doc_dir)?;
        }
        println!("doc_dir: {}", doc_dir.to_str().unwrap());
        for document in documents {
            let document_string = document.render()?;
            let mut document_path =
                Path::new(&format!("{}", doc_dir.to_str().unwrap())).to_path_buf();
            println!("{}", document_path.to_str().unwrap());
            document_path.push(format!("{}.md", document.file));
            println!("{}", document_path.to_str().unwrap());
            if !document_path.parent().unwrap().exists() {
                fs::create_dir(document_path.parent().unwrap())?;
            }
            fs::write(document_path, document_string).expect("Unable to write file");
        }
        Ok(())
    }
}
