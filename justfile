# Build a crate's Docker image
build crate:
    docker build -t {{crate}} -f crates/{{crate}}/Dockerfile .
