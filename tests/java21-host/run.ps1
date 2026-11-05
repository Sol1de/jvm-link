param([Parameter(Mandatory = $true)][string]$JavaHome, [switch]$Jvmti, [switch]$Release)
$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path "$PSScriptRoot\..\..").Path
$outputDir = Join-Path $projectRoot 'target\java21-host'
New-Item -ItemType Directory -Force -Path $outputDir | Out-Null

$featureArgs = @()
if ($Jvmti) { $featureArgs = @('--features', 'jvmti-checks') }
if ($Release) { $featureArgs += '--release' }
$profile = if ($Release) { 'release' } else { 'debug' }
cargo build --locked --manifest-path "$PSScriptRoot\Cargo.toml" --target-dir "$projectRoot\target" @featureArgs
if ($LASTEXITCODE -ne 0) { throw 'Technical JNI adapter build failed' }
& "$JavaHome\bin\javac.exe" -d $outputDir "$PSScriptRoot\DiscoveryHost.java"
if ($LASTEXITCODE -ne 0) { throw 'Technical Java host compilation failed' }
& "$JavaHome\bin\java.exe" -Xcheck:jni -cp $outputDir DiscoveryHost "$projectRoot\target\$profile\jvm_link_discovery_host.dll"
if ($LASTEXITCODE -ne 0) { throw 'Real JVM discovery check failed' }
