use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use std::collections::{BTreeMap, HashMap};

#[derive(Serialize, Deserialize)]
pub(crate) struct DockerComposeSpec {
    pub services: HashMap<String, DockerComposeService>,

    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub networks: HashMap<String, Option<Network>>,

    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub volumes: HashMap<String, Option<Volume>>,
}

#[derive(Serialize, Deserialize, Default)]
pub(crate) struct DockerComposeService {
    pub image: String,

    #[serde(default)]
    pub command: Vec<String>,

    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub entrypoint: Vec<String>,

    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,

    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub environment: HashMap<String, String>,

    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<Port>,

    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<String>,

    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<ServiceVolume>,

    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub depends_on: BTreeMap<String, DependsOn>,
}

#[derive(Default, Debug)]
pub(crate) struct ServiceVolume {
    pub host: String,
    pub target: String,
}

impl Serialize for ServiceVolume {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        format!("{}:{}", self.host, self.target).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ServiceVolume {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ServiceVolumeVisitor;

        impl<'de> Visitor<'de> for ServiceVolumeVisitor {
            type Value = ServiceVolume;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a volume mapping string like '/host/path:/container/path'")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let parts: Vec<&str> = value.split(':').collect();
                if parts.len() >= 2 {
                    Ok(ServiceVolume {
                        host: parts[0].to_string(),
                        target: parts[1].to_string(),
                    })
                } else {
                    Err(E::custom("invalid volume mapping format"))
                }
            }
        }

        deserializer.deserialize_str(ServiceVolumeVisitor)
    }
}

#[derive(Serialize, Deserialize, Default)]
pub(crate) struct Volume {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    #[serde(default)]
    pub driver_opts: HashMap<String, String>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Default)]
pub(crate) struct Network {}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DependsOnCondition {
    ServiceCompletedSuccessfully,
}

#[derive(Default, Debug)]
pub(crate) struct DependsOn {
    pub condition: Option<DependsOnCondition>,
}

impl Serialize for DependsOn {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.condition {
            None => {
                // Serialize as empty map for simple dependency
                ().serialize(serializer)
            }
            Some(condition) => {
                #[derive(Serialize)]
                struct WithCondition<'a> {
                    condition: &'a DependsOnCondition,
                }
                WithCondition { condition }.serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for DependsOn {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WithCondition {
            condition: DependsOnCondition,
        }

        // Try to deserialize as a map with condition
        if let Ok(value) = WithCondition::deserialize(deserializer) {
            Ok(DependsOn {
                condition: Some(value.condition),
            })
        } else {
            // Simple dependency without condition
            Ok(DependsOn { condition: None })
        }
    }
}

#[derive(Debug)]
pub(crate) struct Port {
    pub host: Option<u16>,
    pub container: u16,
}

impl Serialize for Port {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let port_mapping = if let Some(host) = self.host {
            // Docker Compose ports format: "host:container" or extended format
            format!("{}:{}", host, self.container)
        } else {
            format!("{}", self.container)
        };

        // For simple format, just serialize as string
        port_mapping.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Port {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PortVisitor;

        impl<'de> Visitor<'de> for PortVisitor {
            type Value = Port;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a port mapping string like '8080:80' or '80'")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let parts: Vec<&str> = value.split(':').collect();
                match parts.len() {
                    1 => {
                        let container = parts[0]
                            .parse()
                            .map_err(|_| E::custom("invalid container port"))?;
                        Ok(Port {
                            host: None,
                            container,
                        })
                    }
                    2 => {
                        let host = parts[0]
                            .parse()
                            .map_err(|_| E::custom("invalid host port"))?;
                        let container = parts[1]
                            .parse()
                            .map_err(|_| E::custom("invalid container port"))?;
                        Ok(Port {
                            host: Some(host),
                            container,
                        })
                    }
                    _ => Err(E::custom("invalid port mapping format")),
                }
            }
        }

        deserializer.deserialize_str(PortVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_docker_compose_spec_serialize() {
        let mut spec = DockerComposeSpec {
            services: HashMap::new(),
            networks: HashMap::new(),
            volumes: HashMap::new(),
        };

        let mut depends_on = BTreeMap::new();
        depends_on.insert(
            "db".to_string(),
            DependsOn {
                condition: Some(DependsOnCondition::ServiceCompletedSuccessfully),
            },
        );
        depends_on.insert("cache".to_string(), DependsOn { condition: None });

        let service = DockerComposeService {
            image: "test-image:latest".to_string(),
            command: vec![],
            ports: vec![
                Port {
                    host: Some(8080),
                    container: 80,
                },
                Port {
                    host: Some(9000),
                    container: 9000,
                },
                Port {
                    host: None,
                    container: 3000,
                },
            ],
            volumes: vec![ServiceVolume {
                host: "/host/path".to_string(),
                target: "/container/path".to_string(),
            }],
            depends_on,
            ..Default::default()
        };
        spec.services.insert("test-service".to_string(), service);
        spec.networks.insert("test".to_string(), None);

        let mut driver_opts = HashMap::new();
        driver_opts.insert("type".to_string(), "nfs".to_string());

        let mut labels = HashMap::new();
        labels.insert("description".to_string(), "My data volume".to_string());

        let volume = Volume {
            driver: Some("local".to_string()),
            driver_opts,
            labels,
        };
        spec.volumes.insert("mydata".to_string(), Some(volume));

        let yaml = serde_yaml::to_string(&spec).unwrap();
        let expected = include_str!("../fixtures/docker-compose-with-volumes.yaml");
        assert_eq!(yaml.trim(), expected.trim());
    }

    #[test]
    fn test_deserialize_and_serialize_roundtrip() {
        let original_yaml = include_str!("../fixtures/docker-compose-with-volumes.yaml");

        let spec: DockerComposeSpec = serde_yaml::from_str(original_yaml).unwrap();
        let serialized_yaml = serde_yaml::to_string(&spec).unwrap();
        assert_eq!(serialized_yaml.trim(), original_yaml.trim());
    }
}
