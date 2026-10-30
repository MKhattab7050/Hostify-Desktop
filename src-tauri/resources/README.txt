The installer build stages hostify-engine.exe here with scripts/prepare-sidecar.ps1.
That script must succeed before bundling. The directory maps to the installation root,
so the engine sits beside Hostify.exe. Plain shell-only Cargo builds need no engine binary.
