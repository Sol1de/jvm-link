param(
    [Parameter(Mandatory)][ValidateSet('debug', 'release')][string]$BuildProfile,
    [Parameter(Mandatory)][ValidateSet('jni', 'jvmti')][string]$Features
)

$ErrorActionPreference = 'Stop'

$cargoArgs = @('--locked', '--no-default-features')
if ($Features -eq 'jvmti') { $cargoArgs += @('--features', 'jvmti') }
$packageArgs = $cargoArgs.Clone()
if ($BuildProfile -eq 'release') { $cargoArgs += '--release' }
cargo check @cargoArgs
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo clippy --all-targets @cargoArgs -- -D warnings
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo test @cargoArgs
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo doc --no-deps @cargoArgs
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo package @packageArgs
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
