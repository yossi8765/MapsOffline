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
    & (Join-Path $PSScriptRoot "check.ps1")
    if ($LASTEXITCODE -ne 0) { Fail "checks failed (tools\check.ps1)" }
    Write-Host "building..." -ForegroundColor Cyan
    if (Test-Path $exe) { Move-Item $exe "$exe.old" -Force -ErrorAction SilentlyContinue }
    # Panic messages carry source paths: map the user profile (registry sources) and the
    # repository to neutral prefixes so the shipped exe holds no Windows user name.
    $env:RUSTFLAGS = "--remap-path-prefix=$env:USERPROFILE=~ --remap-path-prefix=$root=."
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

# --- Linux: deb + AppImage, built in the WSL distro "mapsoffline-build" (C:\MapsOffline-linux\build-linux.sh)
$linuxOut = "C:\MapsOffline-linux\out"
if (-not $NoBuild -and -not $SignOnly) {
    Write-Host "building Linux (WSL)..." -ForegroundColor Cyan
    $env:WSL_UTF8 = "1"
    wsl -d mapsoffline-build -u builder -- bash /mnt/c/MapsOffline-linux/build-linux.sh | Select-Object -Last 3
    if ($LASTEXITCODE -ne 0) { Fail "Linux build failed" }
}
$linuxFiles = @()
foreach ($ext in @("deb", "AppImage")) {
    $src = Join-Path $linuxOut "MapsOffline_${version}_amd64.$ext"
    if (Test-Path $src) {
        Copy-Item $src (Join-Path $dist "MapsOffline-linux.$ext")
        Copy-Item $src (Join-Path $dist "MapsOffline_${version}_amd64.$ext")
        $linuxFiles += @("MapsOffline-linux.$ext", "MapsOffline_${version}_amd64.$ext")
    }
}
if ($linuxFiles.Count -eq 0) { Write-Host "WARNING: no Linux files for $version in $linuxOut" -ForegroundColor Yellow }

# --- Mac: built by GitHub Actions (.github/workflows/build-mac.yml) when the version tag reaches
# GitHub; wait for that run and take its DMG into the signed list
$github = "yossi8765/MapsOffline"
if ($Replace -and -not $SignOnly) {
    cmd /c "glab release delete $tag -y 2>nul" | Out-Null
    cmd /c "gh release delete $tag -R $github --yes 2>nul" | Out-Null
    cmd /c "git tag -d $tag 2>nul" | Out-Null
    cmd /c "git push origin :refs/tags/$tag 2>nul" | Out-Null
    cmd /c "git push github :refs/tags/$tag 2>nul" | Out-Null
}
$macFiles = @()
if (-not $SignOnly) {
    $exists = & git tag -l $tag
    if (-not $exists) { & git tag -a $tag -m "MapsOffline $version" }
    cmd /c "git push github $tag 2>nul" | Out-Null
    cmd /c "git push origin $tag 2>nul" | Out-Null
    Write-Host "waiting for the Mac build on GitHub..." -ForegroundColor Cyan
    $runId = $null
    for ($i = 0; $i -lt 30 -and -not $runId; $i++) {
        Start-Sleep -Seconds 10
        $runId = (& gh run list -R $github --workflow build-mac.yml --branch $tag --limit 1 --json databaseId --jq ".[0].databaseId" 2>$null)
    }
    if ($runId) {
        & gh run watch $runId -R $github --exit-status | Out-Null
        if ($LASTEXITCODE -eq 0) {
            $macDir = Join-Path $env:TEMP "mapsoffline-mac-$version"
            if (Test-Path $macDir) { Remove-Item $macDir -Recurse -Force }
            & gh run download $runId -R $github -n mac -D $macDir
            $dmg = Get-ChildItem $macDir -Filter *.dmg | Select-Object -First 1
            if ($dmg) {
                Copy-Item $dmg.FullName (Join-Path $dist "MapsOffline-mac.dmg")
                Copy-Item $dmg.FullName (Join-Path $dist "MapsOffline_${version}_universal.dmg")
                $macFiles = @("MapsOffline-mac.dmg", "MapsOffline_${version}_universal.dmg")
            }
        }
    }
    if ($macFiles.Count -eq 0) { Write-Host "WARNING: no Mac build for $version (the release goes out without it)" -ForegroundColor Yellow }
}

# --- SHA256SUMS + signature
$files = @("MapsOffline-setup.exe", "MapsOffline_${version}_x64-setup.exe", "MapsOffline-portable.zip", "MapsOffline_${version}_portable.zip") + $linuxFiles + $macFiles
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
$body = if (Test-Path $notesFile) { Get-Content $notesFile -Raw -Encoding UTF8 } else { "MapsOffline $version" }
$table = "`n`n| קובץ | גודל (בתים) | SHA-256 |`n|---|---:|---|`n"
foreach ($f in $files) {
    $p = Join-Path $dist $f
    $table += "| ``$f`` | $((Get-Item $p).Length) | ``$((Get-FileHash $p -Algorithm SHA256).Hash.ToLower())`` |`n"
}
$body = $body.TrimEnd() + $table
$notesOut = Join-Path $dist "release-notes-$version.md"
[System.IO.File]::WriteAllText($notesOut, $body, (New-Object System.Text.UTF8Encoding $false))

# --- publish (the tag was pushed above, before the Mac build)
$assets = @(
    "$dist\MapsOffline_${version}_x64-setup.exe#MapsOffline_${version}_x64-setup.exe (מתקין)#package",
    "$dist\MapsOffline-setup.exe#MapsOffline-setup.exe (מתקין, שם קבוע)#package",
    "$dist\MapsOffline_${version}_portable.zip#MapsOffline_${version}_portable.zip (נייד, בלי התקנה)#package",
    "$dist\MapsOffline-portable.zip#MapsOffline-portable.zip (נייד, שם קבוע)#package"
) + ($linuxFiles | ForEach-Object {
    $label = if ($_ -like "*.deb") { "התקנה ללינוקס: Ubuntu / Debian / Mint" } else { "לינוקס: קובץ אחד שרץ בלי התקנה" }
    if ($_ -like "MapsOffline-linux*") { $label += ", שם קבוע" }
    "$dist\$_#$_ ($label)#package"
}) + ($macFiles | ForEach-Object {
    $label = if ($_ -like "MapsOffline-mac*") { "מק: Apple silicon ו-Intel, שם קבוע" } else { "מק: Apple silicon ו-Intel" }
    "$dist\$_#$_ ($label)#package"
}) + @(
    "$dist\SHA256SUMS#SHA256SUMS#other",
    "$dist\SHA256SUMS.sig#SHA256SUMS.sig#other"
)
# glab prints "update available" on stderr, which $ErrorActionPreference = "Stop" turns into a failure
$env:GLAB_CHECK_UPDATE = "false"
& glab release create $tag --name "MapsOffline $version" --notes-file $notesOut -R $project @assets
if ($LASTEXITCODE -ne 0) { Fail "glab release create failed" }
Write-Host "published https://gitlab.com/$project/-/releases/$tag" -ForegroundColor Green

# --- the same release on GitHub (the project's home; the app looks there first)
# GitHub gets the fixed names only (the versioned copies are the same files; GitLab keeps both)
$ghAssets = $assets | Where-Object { ($_.Split("#")[0] | Split-Path -Leaf) -notmatch "^MapsOffline_\d" } | ForEach-Object { $p = $_.Split("#"); "$($p[0])#$($p[1] -replace ', שם קבוע', '')" }
& gh release create $tag -R $github --title "MapsOffline $version" --notes-file $notesOut @ghAssets
if ($LASTEXITCODE -ne 0) { Fail "gh release create failed" }
Write-Host "published https://github.com/$github/releases/tag/$tag" -ForegroundColor Green
