# MapsOffline release: build the installer and the portable zip, write SHA256SUMS (first line
# "# version X.Y.Z") signed with the project's Ed25519 key, and publish the GitLab release with
# stable asset names so permalink/latest and the in-app update (src-tauri/src/app_update.rs) work.
#
#   .\tools\release.ps1                 build + sign + publish release vX.Y.Z (version from tauri.conf.json)
#   .\tools\release.ps1 -NoBuild        reuse the existing build outputs
#   .\tools\release.ps1 -Replace        delete the existing release/tag vX.Y.Z first (overwrite)
#   .\tools\release.ps1 -SignOnly       only write SHA256SUMS + SHA256SUMS.sig into dist\
#
# Private key: %USERPROFILE%\.mapsoffline-signing\mapsoffline-release-ed25519.pem (never in the repo).
# Its public half is PUBLIC_KEY_HEX in app_update.rs; the script refuses to sign with a different key.
param(
    [switch]$NoBuild,
    [switch]$Replace,
    [switch]$SignOnly,
    [string]$Notes = "release-notes.md"
)
$ErrorActionPreference = "Stop"
function Fail($m) { Write-Host "ERROR: $m" -ForegroundColor Red; exit 1 }

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$conf = Get-Content (Join-Path $root "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json
$version = $conf.version
if (-not $version) { Fail "no version in src-tauri/tauri.conf.json" }
$tag = "v$version"
$project = "RecoveryAdvancedFree/MapsOffline"
Write-Host "MapsOffline $version -> $tag" -ForegroundColor Cyan

# --- tools
$openssl = (Get-Command openssl -ErrorAction SilentlyContinue).Source
foreach ($c in @("$env:ProgramFiles\Git\mingw64\bin\openssl.exe", "$env:ProgramFiles\Git\usr\bin\openssl.exe")) { if (-not $openssl -and (Test-Path $c)) { $openssl = $c } }
if (-not $openssl) { Fail "openssl not found (install Git for Windows)" }
$key = Join-Path $env:USERPROFILE ".mapsoffline-signing\mapsoffline-release-ed25519.pem"
if (-not (Test-Path $key)) { Fail "signing key missing: $key" }
# public half: DER (last 32 bytes = raw key) for the hex check, PEM for signature verification
$pubDer = Join-Path $env:TEMP "mapsoffline-release-pub.der"
$pubPem = Join-Path $env:TEMP "mapsoffline-release-pub.pem"
& $openssl pkey -in $key -pubout -outform DER -out $pubDer
& $openssl pkey -in $key -pubout -out $pubPem
$derBytes = [System.IO.File]::ReadAllBytes($pubDer)
$pubHex = -join ($derBytes[($derBytes.Length - 32)..($derBytes.Length - 1)] | ForEach-Object { $_.ToString("x2") })
$src = Get-Content (Join-Path $root "src-tauri\src\app_update.rs") -Raw
if ($src -notmatch 'PUBLIC_KEY_HEX: &str = "([0-9a-f]{64})"') { Fail "PUBLIC_KEY_HEX not found in app_update.rs" }
if ($Matches[1] -ne $pubHex) { Fail "the signing key does not match PUBLIC_KEY_HEX in app_update.rs ($pubHex)" }
Write-Host "signing key matches the program's public key" -ForegroundColor Green

# --- build
$setup = Join-Path $root "src-tauri\target\release\bundle\nsis\MapsOffline_${version}_x64-setup.exe"
$exe = Join-Path $root "src-tauri\target\release\mapsoffline.exe"
$dist = Join-Path $root "dist"
if (-not $NoBuild -and -not $SignOnly) {
    Write-Host "building..." -ForegroundColor Cyan
    if (Test-Path $exe) { Move-Item $exe "$exe.old" -Force -ErrorAction SilentlyContinue }
    cmd /c "pnpm tauri build 2>&1" | Select-String -Pattern "error|Finished" | ForEach-Object { $_.Line }
    Remove-Item "$exe.old" -Force -ErrorAction SilentlyContinue
}
if (-not (Test-Path $setup)) { Fail "installer not found: $setup" }
if (-not (Test-Path $exe)) { Fail "executable not found: $exe" }

New-Item -ItemType Directory -Force $dist | Out-Null
Get-ChildItem $dist -File | Remove-Item -Force
$portableDir = Join-Path $dist "MapsOffline"
if (Test-Path $portableDir) { Remove-Item $portableDir -Recurse -Force }
New-Item -ItemType Directory -Force $portableDir | Out-Null
Copy-Item $exe (Join-Path $portableDir "MapsOffline.exe")
"portable" | Set-Content (Join-Path $portableDir "portable.txt") -Encoding ascii
Copy-Item (Join-Path $root "README.md") $portableDir
$portableZip = Join-Path $dist "MapsOffline_${version}_portable.zip"
Compress-Archive -Path $portableDir -DestinationPath $portableZip -Force
Remove-Item $portableDir -Recurse -Force
# stable names (what permalink/latest and the updater download)
Copy-Item $setup (Join-Path $dist "MapsOffline-setup.exe")
Copy-Item $portableZip (Join-Path $dist "MapsOffline-portable.zip")
Copy-Item $setup (Join-Path $dist "MapsOffline_${version}_x64-setup.exe")

# --- SHA256SUMS + signature
$files = @("MapsOffline-setup.exe", "MapsOffline_${version}_x64-setup.exe", "MapsOffline-portable.zip", "MapsOffline_${version}_portable.zip")
$lines = @("# version $version")
foreach ($f in $files) {
    $h = (Get-FileHash (Join-Path $dist $f) -Algorithm SHA256).Hash.ToLower()
    $lines += "$h  $f"
}
$sums = Join-Path $dist "SHA256SUMS"
[System.IO.File]::WriteAllText($sums, ($lines -join "`n") + "`n", (New-Object System.Text.UTF8Encoding $false))
$sig = Join-Path $dist "SHA256SUMS.sig"
& $openssl pkeyutl -sign -inkey $key -rawin -in $sums -out $sig
$verify = cmd /c "`"$openssl`" pkeyutl -verify -pubin -inkey `"$pubPem`" -rawin -in `"$sums`" -sigfile `"$sig`" 2>&1"
if ($LASTEXITCODE -ne 0 -or "$verify" -notmatch "Success") { Fail "signature verification failed: $verify" }
Remove-Item $pubDer, $pubPem -Force -ErrorAction SilentlyContinue
Write-Host "SHA256SUMS written and signed (verified)" -ForegroundColor Green
Get-Content $sums
if ($SignOnly) { exit 0 }

# --- release notes: what's new + checksum table
$notesFile = Join-Path $root $Notes
$body = if (Test-Path $notesFile) { Get-Content $notesFile -Raw } else { "MapsOffline $version" }
$table = "`n`n| קובץ | גודל (בתים) | SHA-256 |`n|---|---:|---|`n"
foreach ($f in $files) {
    $p = Join-Path $dist $f
    $table += "| ``$f`` | $((Get-Item $p).Length) | ``$((Get-FileHash $p -Algorithm SHA256).Hash.ToLower())`` |`n"
}
$body = $body.TrimEnd() + $table
$notesOut = Join-Path $dist "release-notes-$version.md"
[System.IO.File]::WriteAllText($notesOut, $body, (New-Object System.Text.UTF8Encoding $false))

# --- git tag + publish
if ($Replace) {
    cmd /c "glab release delete $tag -y 2>nul" | Out-Null
    cmd /c "git tag -d $tag 2>nul" | Out-Null
    cmd /c "git push origin :refs/tags/$tag 2>nul" | Out-Null
}
$exists = & git tag -l $tag
if (-not $exists) { & git tag -a $tag -m "MapsOffline $version" }
cmd /c "git push origin $tag 2>nul" | Out-Null
$assets = @(
    "$dist\MapsOffline_${version}_x64-setup.exe#MapsOffline_${version}_x64-setup.exe (מתקין)#package",
    "$dist\MapsOffline-setup.exe#MapsOffline-setup.exe (מתקין, שם קבוע)#package",
    "$dist\MapsOffline_${version}_portable.zip#MapsOffline_${version}_portable.zip (נייד, בלי התקנה)#package",
    "$dist\MapsOffline-portable.zip#MapsOffline-portable.zip (נייד, שם קבוע)#package",
    "$dist\SHA256SUMS#SHA256SUMS#other",
    "$dist\SHA256SUMS.sig#SHA256SUMS.sig#other"
)
& glab release create $tag --name "MapsOffline $version" --notes-file $notesOut -R $project @assets
if ($LASTEXITCODE -ne 0) { Fail "glab release create failed" }
Write-Host "published https://gitlab.com/$project/-/releases/$tag" -ForegroundColor Green
