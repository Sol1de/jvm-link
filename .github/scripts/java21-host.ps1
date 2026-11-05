param(
    [Parameter(Mandatory)][ValidateSet('debug', 'release')][string]$BuildProfile,
    [Parameter(Mandatory)][ValidateSet('jni', 'jvmti')][string]$Features
)

$ErrorActionPreference = 'Stop'

$adapterArgs = @('--locked', '--manifest-path', 'tests/java21-host/Cargo.toml')
if ($Features -eq 'jvmti') { $adapterArgs += @('--features', 'jvmti-checks') }
if ($BuildProfile -eq 'release') { $adapterArgs += '--release' }
cargo clippy --lib @adapterArgs -- -D warnings
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$hostArgs = @{ JavaHome = $env:JAVA_HOME }
if ($Features -eq 'jvmti') { $hostArgs.Jvmti = $true }
if ($BuildProfile -eq 'release') { $hostArgs.Release = $true }
./tests/java21-host/run.ps1 @hostArgs
