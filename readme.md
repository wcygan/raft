# Raft

References:

1. https://raft.github.io/
2. https://raft.github.io/raft.pdf

## Buf Configuration

In order to build the project, we need to setup connection to the buf registry.

Add to .cargo/config.toml

```toml
[registries.buf]
index = "sparse+https://buf.build/gen/cargo/"
credential-provider = "cargo:token"
```

Next, login with your token:

```bash
cargo login --registry buf "Bearer {token}"
```

Reference: https://buf.build/docs/bsr/generated-sdks/cargo/