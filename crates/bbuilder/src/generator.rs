use spec::{Artifacts, File, Generated, Manifest, Pod, ResolvedSource, Source, Spec};
use std::collections::HashMap;
use std::path::Path;

pub fn generate(
    manifest: Manifest,
    secrets_dir: &Path,
) -> eyre::Result<Manifest<ResolvedSource>> {
    let deployment = manifest.name.clone();
    let mut pods = HashMap::new();

    for (pod_name, pod) in manifest.pods {
        let mut specs = HashMap::new();
        for (spec_name, spec) in pod.specs {
            specs.insert(spec_name, generate_spec(spec, &deployment, secrets_dir)?);
        }
        pods.insert(pod_name, Pod { specs });
    }

    Ok(Manifest {
        name: manifest.name,
        pods,
    })
}

fn generate_spec(
    spec: Spec,
    deployment: &str,
    secrets_dir: &Path,
) -> eyre::Result<Spec<ResolvedSource>> {
    let mut artifacts = Vec::with_capacity(spec.artifacts.len());

    for artifact in spec.artifacts {
        let Artifacts::File(file) = artifact;
        let source = generate_source(file.source, deployment, &file.name, secrets_dir)?;
        artifacts.push(Artifacts::File(File {
            name: file.name,
            target_path: file.target_path,
            source,
        }));
    }

    Ok(Spec {
        image: spec.image,
        tag: spec.tag,
        args: spec.args,
        entrypoint: spec.entrypoint,
        labels: spec.labels,
        env: spec.env,
        artifacts,
        ports: spec.ports,
        volumes: spec.volumes,
        platform: spec.platform,
        extensions: spec.extensions,
    })
}

fn generate_source(
    source: Source,
    deployment: &str,
    name: &str,
    secrets_dir: &Path,
) -> eyre::Result<ResolvedSource> {
    Ok(match source {
        Source::Inline(content) => ResolvedSource::Inline(content),
        Source::Remote { url, checksum } => ResolvedSource::Remote { url, checksum },
        Source::Jwt => ResolvedSource::Inline(spec::DEFAULT_JWT_TOKEN.to_string()),
        Source::Generated(generated) => {
            ResolvedSource::Inline(secret(generated, deployment, name, secrets_dir)?)
        }
    })
}

fn secret(
    generated: Generated,
    deployment: &str,
    name: &str,
    secrets_dir: &Path,
) -> eyre::Result<String> {
    let dir = secrets_dir.join(deployment);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(name);

    if path.exists() {
        return Ok(std::fs::read_to_string(&path)?);
    }

    let content = match generated {
        Generated::Ed25519TendermintNodeKey => cosmos_keys::generate_tendermint_key().serialize()?,
        Generated::Secp256k1CometBftValidatorKey => {
            cosmos_keys::generate_cometbft_key().serialize()?
        }
    };

    std::fs::write(&path, &content)?;
    tracing::info!("generated secret {} for deployment {}", name, deployment);

    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spec::{Pod, Spec};

    fn manifest_with(file: File) -> Manifest {
        let mut manifest = Manifest::new("test".to_string());
        let spec = Spec::builder()
            .image("test-image")
            .artifact(Artifacts::File(file))
            .build();
        manifest.add_spec("pod".to_string(), Pod::default().with_spec("service", spec));
        manifest
    }

    fn only_source(manifest: Manifest<ResolvedSource>) -> ResolvedSource {
        let spec = &manifest.pods["pod"].specs["service"];
        let Artifacts::File(file) = &spec.artifacts[0];
        file.source.clone()
    }

    #[test]
    fn jwt_resolves_to_the_shared_token() -> eyre::Result<()> {
        let dir = tempdir()?;
        let resolved = generate(manifest_with(File::jwt("jwt", "/data/jwt")), dir.path())?;

        match only_source(resolved) {
            ResolvedSource::Inline(content) => assert_eq!(content, spec::DEFAULT_JWT_TOKEN),
            other => panic!("expected inline, got {:?}", other),
        }
        Ok(())
    }

    #[test]
    fn remote_passes_through_untouched() -> eyre::Result<()> {
        let dir = tempdir()?;
        let file = File::remote("genesis", "/data/genesis.json", "https://example.com/g.json");
        let resolved = generate(manifest_with(file), dir.path())?;

        match only_source(resolved) {
            ResolvedSource::Remote { url, checksum } => {
                assert_eq!(url, "https://example.com/g.json");
                assert_eq!(checksum, None);
            }
            other => panic!("expected remote, got {:?}", other),
        }
        Ok(())
    }

    #[test]
    fn generated_secrets_are_stable_across_runs() -> eyre::Result<()> {
        let dir = tempdir()?;
        let file = || {
            File::generated(
                "node_key.json",
                "/data/node_key.json",
                Generated::Ed25519TendermintNodeKey,
            )
        };

        let first = only_source(generate(manifest_with(file()), dir.path())?);
        let second = only_source(generate(manifest_with(file()), dir.path())?);

        match (first, second) {
            (ResolvedSource::Inline(a), ResolvedSource::Inline(b)) => {
                assert_eq!(a, b, "re-running must not rotate the key");
                assert!(dir.path().join("test").join("node_key.json").exists());
            }
            other => panic!("expected inline, got {:?}", other),
        }
        Ok(())
    }

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tempdir() -> eyre::Result<TempDir> {
        let base = std::env::temp_dir().join(format!("bbuilder-generator-{}", std::process::id()));
        let unique = base.join(format!("{:?}", std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&unique);
        std::fs::create_dir_all(&unique)?;
        Ok(TempDir(unique))
    }
}
