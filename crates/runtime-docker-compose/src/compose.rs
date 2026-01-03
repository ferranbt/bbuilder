use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use std::collections::HashMap;

#[derive(Serialize)]
pub(crate) struct DockerComposeSpec {
    pub services: HashMap<String, DockerComposeService>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub networks: HashMap<String, Option<Network>>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub volumes: HashMap<String, Option<Volume>>,
}

#[derive(Serialize, Default)]
pub(crate) struct DockerComposeService {
    pub image: String,

    pub command: Vec<String>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub entrypoint: Vec<String>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub environment: HashMap<String, String>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<Port>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<String>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(serialize_with = "serialize_service_volume")]
    pub volumes: Vec<ServiceVolume>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    #[serde(serialize_with = "serialize_depends_on")]
    pub depends_on: HashMap<String, Option<DependsOnCondition>>,
}

#[derive(Default)]
pub(crate) struct ServiceVolume {
    pub host: String,
    pub target: String,
}

#[derive(Serialize, Default)]
pub(crate) struct Volume {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub driver_opts: HashMap<String, String>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
}

#[derive(Serialize, Default)]
pub(crate) struct Network {}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DependsOnCondition {
    ServiceCompletedSuccessfully,
}

fn serialize_depends_on<S>(
    map: &HashMap<String, Option<DependsOnCondition>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::Serialize;

    let mut map_serializer = serializer.serialize_map(Some(map.len()))?;
    for (key, value) in map {
        match value {
            None => {
                // Serialize as empty map for simple dependency
                map_serializer.serialize_entry(key, &())?;
            }
            Some(condition) => {
                #[derive(Serialize)]
                struct WithCondition<'a> {
                    condition: &'a DependsOnCondition,
                }
                map_serializer.serialize_entry(key, &WithCondition { condition })?;
            }
        }
    }
    map_serializer.end()
}

fn serialize_service_volume<S>(
    volumes: &Vec<ServiceVolume>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    use serde::ser::SerializeSeq;

    let mut seq = serializer.serialize_seq(Some(volumes.len()))?;
    for volume in volumes {
        let volume_str = format!("{}:{}", volume.host, volume.target);
        seq.serialize_element(&volume_str)?;
    }
    seq.end()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_docker_compose_spec_with_volumes() {
        let mut spec = DockerComposeSpec {
            services: HashMap::new(),
            networks: HashMap::new(),
            volumes: HashMap::new(),
        };

        let service = DockerComposeService {
            image: "test-image:latest".to_string(),
            command: vec![],
            volumes: vec![ServiceVolume {
                host: "/host/path".to_string(),
                target: "/container/path".to_string(),
            }],
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
}
