$ErrorActionPreference = 'Stop'

$metadata = cargo metadata --no-deps --format-version 1 --locked | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$package = $metadata.packages | Where-Object { $_.name -eq 'jvm-link' }
if (-not $package) { throw 'jvm-link package not found in Cargo metadata' }
"version=$($package.version)" >> $env:GITHUB_OUTPUT
$uri = "https://crates.io/api/v1/crates/$($package.name)/$($package.version)"
try {
  $response = Invoke-WebRequest -Uri $uri -Headers @{ 'User-Agent' = 'jvm-link-ci (https://github.com/Sol1de/jvm-link)' } -TimeoutSec 30
  if ($response.StatusCode -ne 200) { throw "Unexpected crates.io response: $($response.StatusCode)" }
  Write-Host "$($package.name) $($package.version) is already published; skipping."
  'publish=false' >> $env:GITHUB_OUTPUT
} catch {
  if ($_.Exception.Response.StatusCode -ne 404) { throw }
  'publish=true' >> $env:GITHUB_OUTPUT
}
