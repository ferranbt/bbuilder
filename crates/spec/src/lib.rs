use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub const DEFAULT_JWT_TOKEN: &str =
    "04592280e1778419b7aa954d43871cb2cfb2ebda754fb735e8adeb293a88f9bf";

#[derive(Debug, Deserialize)]
pub struct Dep {
    pub module: String,
    pub chain: String,
    pub args: serde_json::Value,
}

pub trait Deployment {
    type Input: DeserializeOwned;
    type Chains: Default;

    fn apply(&self, dep: &Dep) -> eyre::Result<Manifest> {
        let input: Self::Input = serde_json::from_value(dep.args.clone())?;
        let manifest = self.manifest(Default::default(), input)?;
        Ok(manifest)
    }

    fn capabilities(&self) -> Vec<ChainSpec<Self::Chains>>;
    fn manifest(&self, chain: Self::Chains, input: Self::Input) -> eyre::Result<Manifest>;
}

pub trait ComputeResource {
    type Chains: Default;

    fn spec(&self, chain: Self::Chains) -> eyre::Result<Pod>;
    fn capabilities(&self) -> Capabilities<Self::Chains>;
}

#[derive(Default)]
pub struct Capabilities<Chains: Default> {
    pub chains: Vec<ChainSpec<Chains>>,
    pub volumes: Vec<Volume>,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct Volume {
    pub name: String,
}

#[derive(Default)]
pub struct ChainSpec<Chains: Default> {
    // full domain name of the chain that the resource can provide compute for
    pub chain: Chains,
    // minimum version of the resource that needs to by used for this chain
    pub min_version: String,
}

pub struct Manifest {
    pub name: String,
    pub pods: HashMap<String, Pod>,
}

impl Manifest {
    pub fn new(name: String) -> Self {
        Manifest {
            name,
            pods: HashMap::new(),
        }
    }

    pub fn add_spec(&mut self, name: String, pod: Pod) {
        self.pods.insert(name, pod);
    }
}

#[derive(Debug, Clone)]
pub enum Artifacts {
    File(File),
}

#[derive(Debug, Clone)]
pub enum Arg {
    Port { name: String, preferred: u16 },
    Dir { name: String, path: String },
    Ref { name: String, port: String },
    File(File),
    Value(String),
}

#[derive(Debug, Clone)]
pub struct Dir {
    pub path: String,
    pub dir: include_dir::Dir<'static>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct File {
    pub name: String,
    pub target_path: String,
    pub content: String,
}

#[macro_export]
macro_rules! port {
    ($name:expr, $port:expr) => {
        spec::Arg::Port {
            name: $name.to_string(),
            preferred: $port,
        }
    };
}

impl From<String> for Arg {
    fn from(s: String) -> Self {
        Arg::Value(s)
    }
}

impl From<&str> for Arg {
    fn from(s: &str) -> Self {
        Arg::Value(s.to_string())
    }
}

impl From<PathBuf> for Arg {
    fn from(path: PathBuf) -> Self {
        Arg::Value(
            path.to_str()
                .expect("Failed to convert path to string")
                .to_string(),
        )
    }
}

impl From<&Path> for Arg {
    fn from(path: &Path) -> Self {
        Arg::Value(
            path.to_str()
                .expect("Failed to convert path to string")
                .to_string(),
        )
    }
}

impl From<&String> for Arg {
    fn from(s: &String) -> Self {
        Arg::Value(s.clone())
    }
}

impl From<&PathBuf> for Arg {
    fn from(path: &PathBuf) -> Self {
        Arg::Value(
            path.to_str()
                .expect("Failed to convert path to string")
                .to_string(),
        )
    }
}

#[derive(Default, Debug, Clone)]
pub struct Pod {
    pub specs: HashMap<String, Spec>,
}

impl Pod {
    pub fn with_spec(mut self, name: &str, spec: impl Into<Spec>) -> Self {
        self.specs.insert(name.to_string(), spec.into());
        self
    }
}

#[derive(Default, Debug, Clone)]
pub struct Spec {
    pub image: String,
    pub tag: Option<String>,
    pub args: Vec<Arg>,
    pub entrypoint: Vec<String>,
    pub labels: HashMap<String, String>,
    pub env: HashMap<String, String>,
    pub artifacts: Vec<Artifacts>,
    pub volumes: HashMap<String, Volume>,
}

#[derive(Default)]
pub struct SpecBuilder {
    image: Option<String>,
    tag: Option<String>,
    args: Vec<Arg>,
    env: HashMap<String, String>,
    entrypoint: Vec<String>,
    labels: HashMap<String, String>,
    artifacts: Vec<Artifacts>,
    volumes: HashMap<String, Volume>,
}

impl Spec {
    pub fn builder() -> SpecBuilder {
        SpecBuilder::default()
    }
}

impl SpecBuilder {
    pub fn image<S: Into<String>>(mut self, image: S) -> Self {
        self.image = Some(image.into());
        self
    }

    pub fn tag<S: Into<String>>(mut self, tag: S) -> Self {
        self.tag = Some(tag.into());
        self
    }

    pub fn arg(mut self, arg: impl Into<Arg>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn arg2(mut self, name: impl Into<String>, value: impl Into<Arg>) -> Self {
        self.args.push(Arg::Value(name.into()));
        self.args.push(value.into());
        self
    }

    pub fn args<I>(mut self, args: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<Arg>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn env<K, V>(mut self, key: K, value: V) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        self.env.insert(key.into(), value.into());
        self
    }

    pub fn entrypoint<I>(mut self, entrypoint: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        self.entrypoint = entrypoint.into_iter().map(|s| s.into()).collect();
        self
    }

    pub fn label<K, V>(mut self, key: K, value: V) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        self.labels.insert(key.into(), value.into());
        self
    }

    pub fn artifact(mut self, artifact: Artifacts) -> Self {
        self.artifacts.push(artifact);
        self
    }

    pub fn volume(mut self, volume: Volume) -> Self {
        self.volumes.insert(volume.name.clone(), volume);
        self
    }

    pub fn build(self) -> Spec {
        Spec {
            image: self.image.unwrap(),
            tag: self.tag,
            args: self.args,
            entrypoint: self.entrypoint,
            labels: self.labels,
            env: self.env,
            artifacts: self.artifacts,
            volumes: self.volumes,
        }
    }
}

impl Into<Spec> for SpecBuilder {
    fn into(self) -> Spec {
        self.build()
    }
}
