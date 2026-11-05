$ErrorActionPreference = 'Stop'

$tag = "v$env:CRATE_VERSION"
$api = "$env:GITHUB_API_URL/repos/$env:GITHUB_REPOSITORY/git"
$headers = @{ Authorization = "Bearer $env:GH_TOKEN"; Accept = 'application/vnd.github+json' }
try {
  $null = Invoke-RestMethod -Uri "$api/ref/tags/$([uri]::EscapeDataString($tag))" -Headers $headers -TimeoutSec 30
  Write-Host "Tag $tag already exists; leaving it unchanged."
  return
} catch {
  if ($_.Exception.Response.StatusCode -ne 404) { throw }
}
$sha = $env:GITHUB_SHA
if ($env:PUBLISH_REQUIRED -eq 'false') {
  $archive = [System.IO.Path]::GetTempFileName()
  try {
    Invoke-WebRequest -Uri "https://crates.io/api/v1/crates/jvm-link/$env:CRATE_VERSION/download" -Headers @{ 'User-Agent' = 'jvm-link-ci (https://github.com/Sol1de/jvm-link)' } -OutFile $archive -TimeoutSec 60
    $vcsJson = tar -xOf $archive "jvm-link-$env:CRATE_VERSION/.cargo_vcs_info.json"
    if ($LASTEXITCODE -ne 0) { throw 'Could not read the published crate commit' }
    $vcs = $vcsJson | ConvertFrom-Json
    $sha = $vcs.git.sha1
    if ($vcs.git.dirty -or $sha -notmatch '^[0-9a-f]{40}$') { throw 'Published crate has no clean Git commit; create its tag manually.' }
  } finally {
    Remove-Item -LiteralPath $archive
  }
}
$body = @{ ref = "refs/tags/$tag"; sha = $sha } | ConvertTo-Json
$null = Invoke-RestMethod -Method Post -Uri "$api/refs" -Headers $headers -ContentType 'application/json' -Body $body -TimeoutSec 30
Write-Host "Created $tag at $sha."
