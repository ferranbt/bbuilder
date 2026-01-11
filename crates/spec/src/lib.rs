use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub const DEFAULT_JWT_TOKEN: &str =
    "04592280e1778419b7aa954d43871cb2cfb2ebda754fb735e8adeb293a88f9bf";

#[derive(Debug, Deserialize)]
pub struct Dep {
    pub name: Option<String>,
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

    fn manifest(&self, chain: Self::Chains, input: Self::Input) -> eyre::Result<Manifest>;
}

pub trait ComputeResource {
    type Chains: Default;

    fn spec(&self, chain: Self::Chains) -> eyre::Result<Pod>;
}

#[derive(Default)]
pub struct Capabilities<Chains: Default> {
    pub chains: Vec<ChainSpec<Chains>>,
    pub volumes: Vec<Volume>,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct Volume {
    pub name: String,
    pub path: String,
}

#[derive(Default)]
pub struct ChainSpec<Chains: Default> {
    // full domain name of the chain that the resource can provide compute for
    pub chain: Chains,
    // minimum version of the resource that needs to by used for this chain
    pub min_version: String,
}

#[derive(Serialize, Deserialize)]
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

    pub fn add_spec(&mut self, name: String, mut pod: Pod) {
        let mut new_specs = Vec::new();
        for (spec_name, spec) in &pod.specs {
            if let Some(babel) = spec.get_babel() {
                let babel_spec = babel.spec();
                let babel_name = format!("{}-babel", spec_name);
                new_specs.push((babel_name, babel_spec));
            }
        }
        for (babel_name, babel_spec) in new_specs {
            pod.specs.insert(babel_name, babel_spec);
        }
        self.pods.insert(name, pod);
    }

    pub fn resolve_ref(&self, name: String, port: String) -> eyre::Result<String> {
        for (pod_name, pod) in &self.pods {
            for (spec_name, spec) in &pod.specs {
                let service_name = format!("{}-{}", pod_name, spec_name);

                if name == service_name {
                    // resolve the port
                    for spec_port in &spec.ports {
                        if spec_port.name == port {
                            return Ok(format!("http://{}:{}", service_name, spec_port.port));
                        }
                    }
                    return Err(eyre::eyre!(
                        "Port '{}' not found in service '{}'",
                        port,
                        service_name
                    ));
                }
            }
        }

        Err(eyre::eyre!("Service '{}' not found", name))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum Artifacts {
    File(File),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Default, Debug, Clone, Deserialize, Serialize)]
pub struct Pod {
    pub specs: HashMap<String, Spec>,
}

impl Pod {
    pub fn with_spec(mut self, name: &str, spec: impl Into<Spec>) -> Self {
        self.specs.insert(name.to_string(), spec.into());
        self
    }
}

#[derive(Default, Debug, Clone, Deserialize, Serialize)]
pub struct Port {
    pub port: u16,
    pub name: String,
}

#[derive(Default, Debug, Clone, Deserialize, Serialize)]
pub struct Spec {
    pub image: String,
    pub tag: Option<String>,
    pub args: Vec<Arg>,
    pub entrypoint: Vec<String>,
    pub labels: HashMap<String, String>,
    pub env: HashMap<String, String>,
    pub artifacts: Vec<Artifacts>,
    pub ports: Vec<Port>,
    pub volumes: Vec<Volume>,
    pub extensions: HashMap<String, serde_json::Value>,
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
    ports: Vec<Port>,
    volumes: Vec<Volume>,
    extensions: HashMap<String, serde_json::Value>,
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
        self.volumes.push(volume);
        self
    }

    pub fn port(mut self, port: Port) -> Self {
        self.ports.push(port);
        self
    }

    pub fn extension<K, V>(mut self, key: K, value: V) -> Self
    where
        K: Into<String>,
        V: Into<serde_json::Value>,
    {
        self.extensions.insert(key.into(), value.into());
        self
    }

    pub fn get_extension<R: serde::de::DeserializeOwned>(
        &self,
        name: String,
    ) -> eyre::Result<Option<R>> {
        match self.extensions.get(&name) {
            Some(value) => Ok(Some(serde_json::from_value(value.clone())?)),
            None => Ok(None),
        }
    }

    pub fn build(self) -> Spec {
        let mut ports = self.ports;

        for arg in &self.args {
            if let Arg::Port { name, preferred } = arg {
                ports.push(Port {
                    port: *preferred,
                    name: name.clone(),
                });
            }
        }

        Spec {
            image: self.image.unwrap(),
            tag: self.tag,
            args: self.args,
            entrypoint: self.entrypoint,
            labels: self.labels,
            env: self.env,
            artifacts: self.artifacts,
            ports: ports,
            volumes: self.volumes,
            extensions: self.extensions,
        }
    }
}

impl Into<Spec> for SpecBuilder {
    fn into(self) -> Spec {
        self.build()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Babel {
    pub node_type: String,
    pub rpc_url: Arg,
}

impl Babel {
    pub fn new(node_type: impl Into<String>, rpc_url: impl Into<Arg>) -> Self {
        Self {
            node_type: node_type.into(),
            rpc_url: rpc_url.into(),
        }
    }

    pub fn spec(self) -> Spec {
        Spec::builder()
            .image("ghcr.io/ferranbt/bbuilder/babel")
            .tag("latest")
            .arg2("--node-type", self.node_type)
            .arg2("--rpc-url", self.rpc_url)
            .arg2("--addr", "0.0.0.0:3000")
            .port(Port {
                port: 3000,
                name: "metrics".to_string(),
            })
            .build()
    }
}

pub trait DeploymentExtension {
    fn min_version(self, version: String) -> Self;
    fn with_babel(self, babel: Babel) -> Self;
    fn get_babel(&self) -> Option<Babel>;
}

impl DeploymentExtension for SpecBuilder {
    fn min_version(self, version: String) -> Self {
        self.extension("min_version", serde_json::Value::String(version))
    }

    fn with_babel(self, babel: Babel) -> Self {
        self.extension("babel", serde_json::to_value(babel).unwrap())
    }

    fn get_babel(&self) -> Option<Babel> {
        self.get_extension("babel".to_string()).ok().flatten()
    }
}

impl DeploymentExtension for Spec {
    fn min_version(self, _version: String) -> Self {
        self
    }

    fn with_babel(self, _babel: Babel) -> Self {
        self
    }

    fn get_babel(&self) -> Option<Babel> {
        self.get_extension("babel".to_string()).ok().flatten()
    }
}

impl Spec {
    pub fn get_extension<R: serde::de::DeserializeOwned>(
        &self,
        name: String,
    ) -> eyre::Result<Option<R>> {
        match self.extensions.get(&name) {
            Some(value) => Ok(Some(serde_json::from_value(value.clone())?)),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arg_port_populates_ports() {
        let spec = Spec::builder()
            .image("test-image")
            .arg(Arg::Port {
                name: "http".to_string(),
                preferred: 8080,
            })
            .arg(Arg::Port {
                name: "grpc".to_string(),
                preferred: 9090,
            })
            .build();

        assert_eq!(spec.ports.len(), 2);
        assert_eq!(spec.ports[0].name, "http");
        assert_eq!(spec.ports[0].port, 8080);
        assert_eq!(spec.ports[1].name, "grpc");
        assert_eq!(spec.ports[1].port, 9090);
    }

    #[test]
    fn test_arg_port_with_existing_ports() {
        let spec = Spec::builder()
            .image("test-image")
            .port(Port {
                port: 3000,
                name: "api".to_string(),
            })
            .arg(Arg::Port {
                name: "http".to_string(),
                preferred: 8080,
            })
            .build();

        assert_eq!(spec.ports.len(), 2);
        assert_eq!(spec.ports[0].name, "api");
        assert_eq!(spec.ports[0].port, 3000);
        assert_eq!(spec.ports[1].name, "http");
        assert_eq!(spec.ports[1].port, 8080);
    }
}
