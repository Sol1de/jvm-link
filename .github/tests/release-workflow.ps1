$ErrorActionPreference = 'Stop'
$workflow = Get-Content "$PSScriptRoot/../workflows/ci.yml" -Raw
$publishJob = [regex]::Match($workflow, '(?ms)^  publish:\r?\n(?<job>.*)\z').Groups['job'].Value
if (-not $publishJob) { throw 'Publication job not found' }
if ($publishJob -notmatch '(?m)^    needs: windows\r?$') { throw 'Publication must wait for the Windows test matrix' }
if ($publishJob -notmatch '(?m)^      queue: max\r?$' -or $publishJob -notmatch '(?m)^      cancel-in-progress: false\r?$') {
    throw 'Publication must retain queued and running jobs'
}

$condition = [regex]::Match($publishJob, '(?m)^    if: (.+)').Groups[1].Value.Trim()
if (-not $condition) { throw 'Publication gate not found' }
$condition = $condition.Replace('github.event_name', '$eventName').Replace('github.ref', '$eventRef').Replace('github.repository', '$repo').Replace(' == ', ' -eq ').Replace(' && ', ' -and ')
$gate = [scriptblock]::Create($condition)
foreach ($eventName in @('push', 'pull_request', 'workflow_dispatch')) {
    foreach ($eventRef in @('refs/heads/main', 'refs/heads/feature', 'refs/tags/v0.1.0')) {
        foreach ($repo in @('Sol1de/jvm-link', 'fork/jvm-link')) {
            $expected = $eventName -eq 'push' -and $eventRef -eq 'refs/heads/main' -and $repo -eq 'Sol1de/jvm-link'
            if ((& $gate) -ne $expected) { throw "Incorrect publication gate: $eventName $eventRef $repo" }
        }
    }
}
Write-Host 'PASS: 18 publication gate scenarios'

foreach ($scriptName in @('check-published-version', 'create-release-tag')) {
    if (-not $publishJob.Contains("run: ./.github/scripts/$scriptName.ps1")) { throw "Release script not called: $scriptName" }
}

class MissingReleaseTag : System.Exception {
    [object] $Response
    MissingReleaseTag() { $this.Response = [pscustomobject]@{ StatusCode = 404 } }
}

function Invoke-RestMethod {
    param($Method, $Uri, $Headers, $TimeoutSec, $ContentType, $Body)
    if ($Method -eq 'Post') {
        if ($Uri -ne 'https://api.github.com/repos/Sol1de/jvm-link/git/refs') { throw "Unexpected write: $Uri" }
        $releaseTest.Created += $Body | ConvertFrom-Json
        return
    }
    if ($Method -and $Method -ne 'Get') { throw "Unexpected HTTP method: $Method" }
    if ($Uri -ne 'https://api.github.com/repos/Sol1de/jvm-link/git/ref/tags/v0.1.0') { throw "Unexpected lookup: $Uri" }
    if ($releaseTest.TagExists) { return @{ object = @{ sha = 'c' * 40 } } }
    throw [MissingReleaseTag]::new()
}

function Invoke-WebRequest {
    param($Uri, $Headers, $OutFile, $TimeoutSec)
    if ($Uri -eq 'https://crates.io/api/v1/crates/jvm-link/0.1.0') {
        if ($releaseTest.RegistryStatus -ne 200) { throw [RegistryFailure]::new($releaseTest.RegistryStatus) }
        return @{ StatusCode = 200 }
    }
    if ($Uri -ne 'https://crates.io/api/v1/crates/jvm-link/0.1.0/download') { throw "Unexpected download: $Uri" }
    $releaseTest.Downloads++
    Copy-Item -LiteralPath $releaseTest.Archive -Destination $OutFile
}

class RegistryFailure : System.Exception {
    [object] $Response
    RegistryFailure([int] $status) { $this.Response = [pscustomobject]@{ StatusCode = $status } }
}

function cargo {
    $global:LASTEXITCODE = 0
    if ($args[0] -eq 'metadata') { return '{"packages":[{"name":"jvm-link","version":"0.1.0"}]}' }
    $releaseTest.CargoCalls += ,@($args)
}

$fixtureDir = Join-Path ([System.IO.Path]::GetTempPath()) ('jvm-link-release-test-' + [guid]::NewGuid().ToString('N'))
$releaseTest = @{}
foreach ($buildProfile in @('debug', 'release')) {
    foreach ($features in @('jni', 'jvmti')) {
        $releaseTest.CargoCalls = @()
        & "$PSScriptRoot/../scripts/validate.ps1" -BuildProfile $buildProfile -Features $features
        if ($releaseTest.CargoCalls.Count -ne 5) { throw 'Validation commands missing' }
        for ($i = 0; $i -lt 5; $i++) {
            $call = $releaseTest.CargoCalls[$i]
            if ($call[0] -ne @('check', 'clippy', 'test', 'doc', 'package')[$i] -or $call -notcontains '--locked' -or $call -notcontains '--no-default-features') { throw 'Validation command changed' }
            if (($call -contains '--release') -ne ($buildProfile -eq 'release' -and $i -ne 4)) { throw 'Incorrect validation profile' }
            if (($call -contains 'jvmti') -ne ($features -eq 'jvmti')) { throw 'Incorrect validation features' }
        }
        Write-Host "PASS: validation $buildProfile/$features"
    }
}
$savedEnv = @{}
foreach ($name in @('GITHUB_API_URL', 'GITHUB_REPOSITORY', 'GITHUB_SHA', 'CRATE_VERSION', 'GH_TOKEN', 'PUBLISH_REQUIRED', 'GITHUB_OUTPUT')) {
    $savedEnv[$name] = [Environment]::GetEnvironmentVariable($name)
}
try {
    $null = New-Item -ItemType Directory -Path "$fixtureDir/jvm-link-0.1.0" -Force
    $fixtureArchive = Join-Path $fixtureDir 'fixture.crate'
    $releaseTest.Archive = $fixtureArchive
    $env:GITHUB_OUTPUT = Join-Path $fixtureDir 'outputs.txt'
    foreach ($registryStatus in @(200, 404, 403, 429, 500)) {
        $releaseTest.RegistryStatus = $registryStatus
        [System.IO.File]::WriteAllText($env:GITHUB_OUTPUT, '')
        $failed = $false
        try { & "$PSScriptRoot/../scripts/check-published-version.ps1" } catch { $failed = $true }
        $outputs = Get-Content $env:GITHUB_OUTPUT
        if ($registryStatus -in @(200, 404)) {
            $expected = if ($registryStatus -eq 200) { 'publish=false' } else { 'publish=true' }
            if ($failed -or $outputs -notcontains 'version=0.1.0' -or $outputs -notcontains $expected) { throw "Incorrect version check: $registryStatus" }
        } elseif (-not $failed -or ($outputs -match '^publish=')) { throw "Registry error ignored: $registryStatus" }
        Write-Host "PASS: registry response $registryStatus"
    }
    $vcs = @{ git = @{ sha1 = 'a' * 40 }; path_in_vcs = '' } | ConvertTo-Json
    [System.IO.File]::WriteAllText("$fixtureDir/jvm-link-0.1.0/.cargo_vcs_info.json", $vcs)
    tar -czf $fixtureArchive -C $fixtureDir jvm-link-0.1.0
    if ($LASTEXITCODE -ne 0) { throw 'Could not create fixture archive' }

    $env:GITHUB_API_URL = 'https://api.github.com'
    $env:GITHUB_REPOSITORY = 'Sol1de/jvm-link'
    $env:GITHUB_SHA = 'b' * 40
    $env:CRATE_VERSION = '0.1.0'
    $env:GH_TOKEN = 'test-token'
    foreach ($case in @('new-publication', 'published-version', 'existing-tag')) {
        $releaseTest.Created = @()
        $releaseTest.Downloads = 0
        $tagExists = $case -eq 'existing-tag'
        $releaseTest.TagExists = $tagExists
        $env:PUBLISH_REQUIRED = if ($case -eq 'new-publication') { 'true' } else { 'false' }
        & "$PSScriptRoot/../scripts/create-release-tag.ps1"
        if ($tagExists) {
            if ($releaseTest.Created.Count -ne 0 -or $releaseTest.Downloads -ne 0) { throw 'Existing tag was modified' }
        } else {
            $expectedSha = if ($case -eq 'new-publication') { 'b' * 40 } else { 'a' * 40 }
            $expectedDownloads = if ($case -eq 'new-publication') { 0 } else { 1 }
            if ($releaseTest.Created.Count -ne 1 -or $releaseTest.Created[0].ref -ne 'refs/tags/v0.1.0' -or $releaseTest.Created[0].sha -ne $expectedSha -or $releaseTest.Downloads -ne $expectedDownloads) {
                throw "Incorrect release tag: $case"
            }
        }
        Write-Host "PASS: $case"
    }
} finally {
    foreach ($name in $savedEnv.Keys) { [Environment]::SetEnvironmentVariable($name, $savedEnv[$name]) }
    $resolved = [System.IO.Path]::GetFullPath($fixtureDir)
    $tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
    if (-not $resolved.StartsWith($tempRoot, [System.StringComparison]::OrdinalIgnoreCase) -or (Split-Path $resolved -Leaf) -notlike 'jvm-link-release-test-*') {
        throw 'Unsafe fixture cleanup path'
    }
    if (Test-Path -LiteralPath $resolved) { Remove-Item -LiteralPath $resolved -Recurse -Force }
}
