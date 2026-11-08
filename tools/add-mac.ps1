# Adds the Mac DMG of a finished GitHub Actions run (build-mac.yml) to an already published
# release, when release.ps1 went out without it (the Mac build failed or was late):
# downloads the signed SHA256SUMS of the release, checks its signature, adds the DMG lines, signs
# again and uploads the DMG + the new list to the GitHub and GitLab releases.
#
#   .\tools\add-mac.ps1 -RunId <build-mac run id>      (version from tauri.conf.json)
param(
    [Parameter(Mandatory = $true)][string]$RunId
)
$ErrorActionPreference = "Continue"
function Fail($m) { Write-Host "ERROR: $m" -ForegroundColor Red; exit 1 }
$root = Split-Path -Parent $PSScriptRoot
$version = (Get-Content (Join-Path $root "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).version
$tag = "v$version"
$github = "yossi8765/MapsOffline"
$gitlab = "RecoveryAdvancedFree/MapsOffline"
$gitlabId = "RecoveryAdvancedFree%2FMapsOffline"
$env:GLAB_CHECK_UPDATE = "false"

$openssl = (Get-Command openssl -ErrorAction SilentlyContinue).Source
foreach ($c in @("$env:ProgramFiles\Git\mingw64\bin\openssl.exe", "$env:ProgramFiles\Git\usr\bin\openssl.exe")) { if (-not $openssl -and (Test-Path $c)) { $openssl = $c } }
if (-not $openssl) { Fail "openssl not found" }
$key = Join-Path $env:USERPROFILE ".mapsoffline-signing\mapsoffline-release-ed25519.pem"
if (-not (Test-Path $key)) { Fail "signing key missing: $key" }

$stage = Join-Path $env:TEMP "mapsoffline-add-mac-$version"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force $stage | Out-Null
$pubPem = Join-Path $stage "pub.pem"
& $openssl pkey -in $key -pubout -out $pubPem

# the DMG
& gh run download $RunId -R $github -n mac -D (Join-Path $stage "run")
$dmg = Get-ChildItem (Join-Path $stage "run") -Filter *.dmg | Select-Object -First 1
if (-not $dmg) { Fail "run $RunId has no Mac DMG" }
$stable = "MapsOffline-mac.dmg"
$versioned = "MapsOffline_${version}_universal.dmg"
Copy-Item $dmg.FullName (Join-Path $stage $stable)
Copy-Item $dmg.FullName (Join-Path $stage $versioned)

# the current signed list (GitHub's: the same as GitLab's)
$old = Join-Path $stage "old"
& gh release download $tag -R $github -p SHA256SUMS -p SHA256SUMS.sig -D $old
$oldSums = Join-Path $old "SHA256SUMS"
$v = cmd /c "`"$openssl`" pkeyutl -verify -pubin -inkey `"$pubPem`" -rawin -in `"$oldSums`" -sigfile `"$old\SHA256SUMS.sig`" 2>&1"
if ("$v" -notmatch "Success") { Fail "the release's SHA256SUMS signature is not valid: $v" }
$lines = (Get-Content $oldSums) | Where-Object { $_ -and $_ -notmatch "\.dmg$" }
if ($lines[0] -ne "# version $version") { Fail "SHA256SUMS is for '$($lines[0])', not $version" }
$h = (Get-FileHash $dmg.FullName -Algorithm SHA256).Hash.ToLower()
$lines += "$h  $stable"
$lines += "$h  $versioned"
$sums = Join-Path $stage "SHA256SUMS"
[System.IO.File]::WriteAllText($sums, ($lines -join "`n") + "`n", (New-Object System.Text.UTF8Encoding $false))
$sig = Join-Path $stage "SHA256SUMS.sig"
& $openssl pkeyutl -sign -inkey $key -rawin -in $sums -out $sig
$v = cmd /c "`"$openssl`" pkeyutl -verify -pubin -inkey `"$pubPem`" -rawin -in `"$sums`" -sigfile `"$sig`" 2>&1"
if ("$v" -notmatch "Success") { Fail "signature verification failed: $v" }
Write-Host "new SHA256SUMS (signed, verified):" -ForegroundColor Green
Get-Content $sums

# GitHub
& gh release upload $tag -R $github --clobber "$stage\$stable#$stable (מק: Apple silicon ו-Intel)" $sums $sig
if ($LASTEXITCODE) { Fail "GitHub upload failed" }
Write-Host "GitHub: https://github.com/$github/releases/tag/$tag" -ForegroundColor Green

# GitLab: links named like the new files are replaced
$links = (& glab api "projects/$gitlabId/releases/$tag/assets/links" 2>$null) | ConvertFrom-Json
foreach ($l in $links) {
    foreach ($n in @($stable, $versioned, "SHA256SUMS", "SHA256SUMS.sig")) {
        if ($l.name -eq $n -or $l.name.StartsWith("$n ")) {
            & glab api -X DELETE "projects/$gitlabId/releases/$tag/assets/links/$($l.id)" 2>&1 | Out-Null
        }
    }
}
& glab release upload $tag -R $gitlab "$stage\$versioned#$versioned (מק: Apple silicon ו-Intel)#package" "$stage\$stable#$stable (מק, שם קבוע)#package" "$sums#SHA256SUMS#other" "$sig#SHA256SUMS.sig#other"
if ($LASTEXITCODE) { Fail "GitLab upload failed - run this script again" }
Write-Host "GitLab: https://gitlab.com/$gitlab/-/releases/$tag" -ForegroundColor Green
