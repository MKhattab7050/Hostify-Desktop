cargo vendor vendor | Out-Null
New-Item -ItemType Directory -Force .cargo | Out-Null
@"
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
"@ | Set-Content .cargo/config.toml
