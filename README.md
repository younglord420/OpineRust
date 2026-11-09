# OpineRust Runtime

Runtime Pine Script® dalam Rust, dipakai oleh Oracly.

Berasal dari [Pinecone](https://github.com/ferranbt/pinecone) dan berlisensi MPL-2.0.
Pine Script® adalah merek TradingView; proyek ini tidak berafiliasi dengan TradingView.

## Build

Butuh Rust 1.83 atau lebih baru.

```sh
cargo test --lib
cargo test -p pine-integration-tests --test integration
```

Nama crate tetap `pine-*` walau produknya OpineRust, supaya API tidak berubah.

## Lisensi

MPL-2.0. Lihat [LICENSE](LICENSE) dan [NOTICE](NOTICE).
