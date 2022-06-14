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
use ethers::abi::{Abi, Param, ParamType, StateMutability};
use ethers::solc::artifacts::{
    output_selection::OutputSelection, Contract, DevDoc, MethodDoc as SolcMethodDoc, UserDoc,
    UserDocNotice,
};
use forge::executor::opts::EvmOpts;
use foundry_common::evm::EvmArgs;
use foundry_config::{figment::Figment, Config};
use globset::Glob;
use regex::Regex;
use std::collections::BTreeMap;
use std::{
    fmt,
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

#[derive(Template, Debug)]
#[template(path = "doc.md")]
struct FileDoc {
    name: String,
    contracts: Vec<ContractDoc>,
}

impl FileDoc {
    fn new(name: String, contracts: &Vec<(String, &Contract)>) -> Self {
        Self {
            name,
            contracts: contracts
                .iter()
                .map(|(name, contract)| ContractDoc::new(name, contract))
                .collect(),
        }
    }
}

// TODO: include internal functions in the output. Would need a rewrite with AST parsing (with
// `fmt`'s visitor implementation, since ABI only contains external functions).

/// Combination of a contract's Abi, UserDoc, DevDoc
#[derive(Debug)]
struct ContractDoc {
    name: String,
    title: Option<String>,
    details: Option<String>,
    notice: Option<String>,
    author: Option<String>,
    // TODO: deal with constructor, receiver function and fallback
    methods: BTreeMap<String, Vec<MethodDoc>>,
    events: BTreeMap<String, Vec<EventDoc>>,
    errors: BTreeMap<String, Vec<ErrorDoc>>,
}

impl ContractDoc {
    fn new(name: &String, contract: &Contract) -> Self {
        let abi = &contract.abi.as_ref().unwrap().abi;
        let devdoc = &contract.devdoc;
        let userdoc = &contract.userdoc;
        Self {
            name: name.to_string(),
            title: devdoc.title.clone(),
            details: devdoc.details.clone(),
            notice: userdoc.notice.clone(),
            author: devdoc.author.clone(),
            methods: Self::parse_methods(abi, &devdoc, &userdoc),
            events: Self::parse_events(abi, &devdoc, &userdoc),
            errors: Self::parse_errors(abi, &devdoc, &userdoc),
        }
    }

    fn parse_methods(
        abi: &Abi,
        dev_doc: &DevDoc,
        user_doc: &UserDoc,
    ) -> BTreeMap<String, Vec<MethodDoc>> {
        let mut methods: BTreeMap<String, Vec<MethodDoc>> = BTreeMap::new();
        for function in abi.functions() {
            let signature = function.signature();
            let signature = signature.split(':').next().unwrap();
            let function_dev_doc =
                dev_doc.methods.get(signature).cloned().unwrap_or(SolcMethodDoc::default());
            let function_user_doc = user_doc.methods.get(signature);
            let params = Self::parse_params(&function.inputs, &function_dev_doc.params);
            let returns = Self::parse_params(&function.outputs, &function_dev_doc.returns);
            methods.entry(function.name.clone()).or_insert(Vec::new()).push(MethodDoc {
                name: function.name.clone(),
                details: function_dev_doc.details.clone(),
                notice: match function_user_doc {
                    Some(UserDocNotice::Constructor(x)) => Some(x.clone()),
                    Some(UserDocNotice::Notice { notice: x }) => Some(x.clone()),
                    None => None,
                },
                state_mutability: function.state_mutability,
                params,
                returns,
            })
        }
        methods
    }

    fn parse_events(
        abi: &Abi,
        devdoc: &DevDoc,
        userdoc: &UserDoc,
    ) -> BTreeMap<String, Vec<EventDoc>> {
        let events: BTreeMap<String, Vec<EventDoc>> = BTreeMap::new();
        events
    }

    fn parse_errors(
        abi: &Abi,
        devdoc: &DevDoc,
        userdoc: &UserDoc,
    ) -> BTreeMap<String, Vec<ErrorDoc>> {
        let errors: BTreeMap<String, Vec<ErrorDoc>> = BTreeMap::new();
        errors
    }

    fn parse_params(params: &Vec<Param>, param_docs: &BTreeMap<String, String>) -> Vec<ParamDoc> {
        params
            .iter()
            .map(|p| ParamDoc {
                name: if p.name.is_empty() { String::from("-") } else { p.name.clone() },
                kind: p.kind.clone(),
                internal_type: p.internal_type.clone(),
                indexed: None,
                doc: param_docs.get(&p.name.clone()).cloned().unwrap_or(String::from("-")),
            })
            .collect()
    }
}

#[derive(Debug)]
struct MethodDoc {
    name: String,
    details: Option<String>,
    notice: Option<String>,
    state_mutability: StateMutability,
    params: Vec<ParamDoc>,
    returns: Vec<ParamDoc>,
}

impl fmt::Display for MethodDoc {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let state_mutability = match self.state_mutability {
            StateMutability::Pure => " pure".to_string(),
            StateMutability::View => " view".to_string(),
            StateMutability::NonPayable => " nonpayable".to_string(),
            StateMutability::Payable => " payable".to_string(),
        };
        let params =
            self.params.iter().map(|x| format!("{}", x)).collect::<Vec<String>>().join(", ");
        let returns = if self.returns.len() > 0 {
            format!(
                " returns ({})",
                self.returns.iter().map(|x| format!("{}", x)).collect::<Vec<String>>().join(", ")
            )
        } else {
            String::new()
        };
        write!(f, "{}({}) external{}{}", self.name, params, state_mutability, returns)
    }
}

#[derive(Debug)]
struct EventDoc {
    name: String,
    details: Option<String>,
    notice: Option<String>,
    params: Vec<ParamDoc>,
}

#[derive(Debug)]
struct ErrorDoc {
    name: String,
    details: Option<String>,
    notice: Option<String>,
    params: Vec<ParamDoc>,
}

#[derive(Debug)]
struct ParamDoc {
    name: String,
    kind: ParamType,
    internal_type: Option<String>,
    /// for Event params
    indexed: Option<bool>,
    doc: String,
}

impl fmt::Display for ParamDoc {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.name.is_empty() {
            write!(f, "{}", self.kind)
        } else {
            write!(f, "{} {}", self.kind, self.name)
        }
    }
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
            BTreeMap::from([(
                "*".to_string(),
                vec!["abi".to_string(), "devdoc".to_string(), "userdoc".to_string()],
            )]),
        )]));
        let compiler = ProjectCompiler::default();
        let output = if self.opts.silent {
            compile::suppress_compile(&project)
        } else {
            compiler.compile(&project)
        }?;

        let src_dir = config.src.to_str().unwrap();
        let src_dir_glob = Glob::new(format!("{}/*", src_dir).as_str())?;
        // let documents: Vec<Document> = output
        // output
        //     .output()
        //     .contracts
        //     .0
        //     .iter()
        //     .filter(|(file, _)| src_dir_glob.compile_matcher().is_match(file))
        //     // .map(|(file, contracts)| Document {
        //     //     file: file
        //     //         .to_string()
        //     //         .strip_prefix(format!("{}/", src_dir).as_str())
        //     //         .unwrap()
        //     //         .strip_suffix(".sol")
        //     //         .unwrap()
        //     //         .to_string(),
        //     //     contracts: contracts
        //     //         .iter()
        //     //         .map(|(name, contract)| Contract {
        //     //             name: name.to_string(),
        //     //             userdoc: contract[0].clone().contract.userdoc,
        //     //             devdoc: contract[0].clone().contract.devdoc,
        //     //             abi: contract[0].clone().contract.abi.unwrap().abi,
        //     //         })
        //     //         .collect(),
        //     // })
        //     // .collect();
        //     .for_each(|(file, contracts)| {
        //         println!(
        //             "{}",
        //             file.to_string()
        //                 .strip_prefix(format!("{}/", src_dir).as_str())
        //                 .unwrap()
        //                 .strip_suffix(".sol")
        //                 .unwrap()
        //                 .to_string()
        //         );
        //         for (name, contract) in contracts {
        //             println!("{}", name);
        //             println!("{:?}", contract[0].clone().contract.abi.unwrap().abi);
        //             println!("{:?}", contract[0].clone().contract.devdoc);
        //             println!("{:?}", contract[0].clone().contract.userdoc);
        //         }
        //     });
        let output = output.output();
        let mut grouped_contracts: BTreeMap<String, Vec<(String, &Contract)>> = BTreeMap::new();
        for (file, name, contract) in output.contracts_with_files_iter() {
            if !src_dir_glob.compile_matcher().is_match(file) {
                continue;
            }
            grouped_contracts
                .entry(
                    file.to_string()
                        .strip_prefix(format!("{}/", src_dir).as_str())
                        .unwrap()
                        .strip_suffix(".sol")
                        .unwrap()
                        .to_string()
                        .into(),
                )
                .or_insert(Vec::new())
                .push((name.into(), &contract));
        }
        let documents: Vec<FileDoc> = grouped_contracts
            .iter()
            .map(|(file, contracts)| FileDoc::new(file.to_string(), contracts))
            .collect();

        let mut doc_dir = PathBuf::new();
        doc_dir.push(src_dir);
        doc_dir.pop();
        doc_dir.push("docs");
        let doc_dir = doc_dir.as_path();
        if !doc_dir.exists() {
            fs::create_dir(doc_dir)?;
        }
        for document in documents {
            let document_string = document.render()?;
            let mut document_path =
                Path::new(&format!("{}", doc_dir.to_str().unwrap())).to_path_buf();
            document_path.push(format!("{}.md", document.name));
            if !document_path.parent().unwrap().exists() {
                fs::create_dir(document_path.parent().unwrap())?;
            }
            fs::write(document_path, document_string).expect("Unable to write file");
        }
        Ok(())
    }
}
