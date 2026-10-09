`mist` CLI sidecar binaries go here, named `mist-<target-triple>[.exe]`
(Tauri `externalBin` convention). They are build outputs and are not
committed; `scripts/prepare-sidecar.sh` / `.ps1` copy them from `target/`.
