# Citadel installer for Windows (PowerShell 5.1 or later).
#
#   irm https://sepehrmohseni.github.io/citadel/install.ps1 | iex
#
# With options:
#
#   & ([scriptblock]::Create((irm https://sepehrmohseni.github.io/citadel/install.ps1))) -Version 0.1.0
#   & ([scriptblock]::Create((irm https://sepehrmohseni.github.io/citadel/install.ps1))) -Uninstall
#
# Environment: CITADEL_VERSION (same as -Version), CITADEL_ASSETS_DIR (install
# from a local directory holding the release assets and SHA256SUMS).
# Documentation: https://github.com/sepehrmohseni/citadel/blob/master/docs/INSTALL.md
#
# Everything is defined inside one function that is called on the last line,
# so a truncated download fails to parse instead of running halfway.

param(
    [string]$Version = $env:CITADEL_VERSION,
    [string]$AssetsDir = $env:CITADEL_ASSETS_DIR,
    [switch]$Uninstall
)

function Invoke-CitadelInstaller {
    param(
        [string]$Version,
        [string]$AssetsDir,
        [switch]$Uninstall
    )

    Set-StrictMode -Version 2.0
    $ErrorActionPreference = 'Stop'

    $Repo = 'sepehrmohseni/citadel'
    $AppId = 'io.github.sepehrmohseni.citadel'
    $Product = 'Citadel'
    $ExeName = 'citadel.exe'
    $UninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Citadel'
    $UserAgent = 'citadel-install-ps1'

    if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) { throw 'citadel: error: install.ps1 is for Windows; use install.sh on Linux and macOS' }
    if ($PSVersionTable.PSVersion.Major -lt 5) { throw 'citadel: error: PowerShell 5.1 or later is required' }

    function Say([string]$Message) { Write-Host "citadel: $Message" }
    function Warn([string]$Message) { Write-Warning "citadel: $Message" }
    function Fail([string]$Message) { throw "citadel: error: $Message" }

    function Test-SafeName([string]$Name) {
        if (-not ($Name -match '^[A-Za-z0-9_+-][A-Za-z0-9._+-]*$')) { Fail "refusing unexpected name '$Name'" }
    }

    function Get-RegValue([string]$Path, [string]$Name) {
        $item = Get-Item -LiteralPath $Path -ErrorAction SilentlyContinue
        if ($null -eq $item) { return $null }
        $value = $item.GetValue($Name)
        if ($value -is [string]) { return $value.Trim('"') }
        return $value
    }

    function Get-Download([string]$Url, [string]$OutFile) {
        if (-not $Url.StartsWith('https://')) { Fail "refusing non-https URL $Url" }
        Invoke-WebRequest -Uri $Url -OutFile $OutFile -UserAgent $UserAgent -UseBasicParsing
    }

    function Invoke-AndWait([string]$FilePath, [string]$Arguments) {
        $proc = Start-Process -FilePath $FilePath -ArgumentList $Arguments -PassThru
        $null = $proc.Handle
        $proc.WaitForExit()
        return $proc.ExitCode
    }

    function Get-StatusCode($ErrorRecord) {
        $response = $null
        if ($ErrorRecord.Exception.PSObject.Properties['Response']) { $response = $ErrorRecord.Exception.Response }
        if ($null -eq $response) { return 0 }
        return [int]$response.StatusCode
    }

    function Resolve-Tag {
        if ($Version) {
            $tag = if ($Version.StartsWith('v')) { $Version } else { "v$Version" }
            Test-SafeName $tag
            return $tag
        }
        $api = "https://api.github.com/repos/$Repo/releases/latest"
        try {
            $release = Invoke-RestMethod -Uri $api -UserAgent $UserAgent -UseBasicParsing
            $tag = [string]$release.tag_name
        } catch {
            $code = Get-StatusCode $_
            if ($code -eq 404) {
                Fail "no published release found for $Repo (drafts and pre-releases are skipped; use -Version to pick one)"
            }
            Warn "GitHub API request failed (HTTP $code); resolving the latest release through github.com instead"
            $request = [System.Net.WebRequest]::Create("https://github.com/$Repo/releases/latest")
            $request.Method = 'HEAD'
            $request.UserAgent = $UserAgent
            $response = $request.GetResponse()
            $final = $response.ResponseUri.AbsoluteUri
            $response.Close()
            if ($final -match '/releases/tag/([^/?#]+)$') { $tag = $Matches[1] } else { Fail "no published release found for $Repo" }
        }
        if (-not $tag) { Fail "could not read the latest release tag from $api" }
        Test-SafeName $tag
        return $tag
    }

    function Read-Sums([string]$Path) {
        $sums = @{}
        foreach ($line in Get-Content -LiteralPath $Path) {
            if ($line -match '^([0-9A-Fa-f]{64}) [ *](\S+)$') { $sums[$Matches[2]] = $Matches[1].ToLowerInvariant() }
        }
        return $sums
    }

    function Find-Asset($Sums, [string]$Suffix) {
        $Sums.Keys | Where-Object { $_.StartsWith("${Product}_") -and $_.EndsWith($Suffix) } | Sort-Object | Select-Object -First 1
    }

    function Get-RunningCitadel([string]$InstallDir) {
        Get-Process -Name 'citadel' -ErrorAction SilentlyContinue | Where-Object {
            $path = $null
            try { $path = $_.Path } catch { }
            (-not $InstallDir) -or (-not $path) -or $path.StartsWith($InstallDir, [StringComparison]::OrdinalIgnoreCase)
        }
    }

    function Get-ShortcutTarget([string]$Path) {
        if (-not (Test-Path -LiteralPath $Path)) { return $null }
        $shell = New-Object -ComObject WScript.Shell
        return $shell.CreateShortcut($Path).TargetPath
    }

    $startMenuLink = Join-Path ([Environment]::GetFolderPath('Programs')) "$Product.lnk"
    $desktopLink = Join-Path ([Environment]::GetFolderPath('Desktop')) "$Product.lnk"
    $dataDirs = @((Join-Path $env:APPDATA $AppId), (Join-Path $env:LOCALAPPDATA $AppId))

    if ($Uninstall) {
        $uninstaller = Get-RegValue $UninstallKey 'UninstallString'
        if (-not $uninstaller) {
            $msi = Get-ChildItem 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall' -ErrorAction SilentlyContinue |
                Where-Object { $_.GetValue('DisplayName') -eq $Product -and $_.GetValue('Publisher') -eq 'Sepehr Mohseni' } |
                Select-Object -First 1
            if ($msi) {
                Say "Citadel was installed machine-wide from the MSI; remove it from Settings > Apps or run, as administrator:"
                Say "  msiexec /x $($msi.PSChildName)"
            } else {
                Say "no per-user Citadel install found"
            }
            return
        }
        $installDir = Split-Path -Parent $uninstaller
        if (-not (Test-Path -LiteralPath $uninstaller)) { Fail "uninstaller not found: $uninstaller" }
        if (Get-RunningCitadel $installDir) { Fail "Citadel is running; close it and rerun the uninstaller" }
        $links = @($startMenuLink, $desktopLink) | Where-Object { Test-Path -LiteralPath $_ }
        Say "running $uninstaller /S"
        $code = Invoke-AndWait $uninstaller "/S _?=$installDir"
        if ($code -ne 0) { Fail "uninstaller exited with code $code" }
        Remove-Item -LiteralPath $uninstaller -Force -ErrorAction SilentlyContinue
        if ((Test-Path -LiteralPath $installDir) -and -not (Get-ChildItem -LiteralPath $installDir -Force)) {
            Remove-Item -LiteralPath $installDir -Force
        }
        if (Test-Path -LiteralPath $UninstallKey) { Warn "the uninstall registry entry is still present: $UninstallKey" }
        foreach ($link in $links) {
            if (Test-Path -LiteralPath $link) { Warn "shortcut still present: $link" } else { Say "removed $link" }
        }
        if (Test-Path -LiteralPath $installDir) { Warn "files remain in $installDir" } else { Say "removed $installDir" }
        Say "kept your vault and settings in $($dataDirs -join ' and '); delete them by hand to erase all data"
        return
    }

    $arch = $env:PROCESSOR_ARCHITECTURE
    if ($env:PROCESSOR_ARCHITEW6432) { $arch = $env:PROCESSOR_ARCHITEW6432 }
    switch ($arch) {
        'AMD64' { $suffixes = @('_x64-setup.exe') }
        'ARM64' { $suffixes = @('_arm64-setup.exe', '_x64-setup.exe') }
        default { Fail "unsupported processor architecture '$arch'; Citadel needs 64-bit Windows" }
    }

    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    $oldProgress = $ProgressPreference
    $ProgressPreference = 'SilentlyContinue'
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("citadel-install-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $sumsFile = Join-Path $tmp 'SHA256SUMS'
        if ($AssetsDir) {
            $AssetsDir = (Resolve-Path -LiteralPath $AssetsDir).Path
            $localSums = Join-Path $AssetsDir 'SHA256SUMS'
            if (-not (Test-Path -LiteralPath $localSums)) { Fail "no SHA256SUMS in CITADEL_ASSETS_DIR=$AssetsDir" }
            Copy-Item -LiteralPath $localSums -Destination $sumsFile
            Say "using local assets from $AssetsDir"
            $tag = 'local'
        } else {
            $tag = Resolve-Tag
            $base = "https://github.com/$Repo/releases/download/$tag"
            try {
                Get-Download "$base/SHA256SUMS" $sumsFile
            } catch {
                Fail "release $tag not found or it has no SHA256SUMS; refusing to install unverified files"
            }
            Say "release $tag"
        }

        $sums = Read-Sums $sumsFile
        $asset = $null
        foreach ($suffix in $suffixes) {
            $asset = Find-Asset $sums $suffix
            if ($asset) { break }
        }
        if (-not $asset) { Fail "release $tag has no Windows installer (*$($suffixes -join ', *'))" }
        Test-SafeName $asset
        if ($arch -eq 'ARM64' -and $asset.EndsWith('_x64-setup.exe')) { Say "no ARM64 build in this release; installing the x64 build, which runs under emulation" }

        $setup = Join-Path $tmp $asset
        if ($AssetsDir) {
            $localAsset = Join-Path $AssetsDir $asset
            if (-not (Test-Path -LiteralPath $localAsset)) { Fail "$asset is listed in SHA256SUMS but missing from $AssetsDir" }
            Copy-Item -LiteralPath $localAsset -Destination $setup
        } else {
            Say "downloading $asset"
            Get-Download "$base/$asset" $setup
        }
        $want = $sums[$asset]
        $got = (Get-FileHash -Algorithm SHA256 -LiteralPath $setup).Hash.ToLowerInvariant()
        if ($got -ne $want) { Fail "checksum mismatch for ${asset}: expected $want, got $got" }
        Say "sha256 ok  $got  $asset"

        $previous = Get-RegValue $UninstallKey 'InstallLocation'
        if (Get-RunningCitadel $previous) { Fail "Citadel is running; close it and rerun the installer" }

        Say "running $asset /S (per-user install, no administrator rights needed)"
        Say "the installer is not code-signed; its SHA-256 was checked against the release's SHA256SUMS instead"
        $code = Invoke-AndWait $setup '/S'
        if ($code -ne 0) { Fail "installer exited with code $code" }

        $installDir = Get-RegValue $UninstallKey 'InstallLocation'
        $installed = Get-RegValue $UninstallKey 'DisplayVersion'
        if (-not $installDir) { Fail "installer finished but no uninstall entry was written to $UninstallKey" }
        $exe = Join-Path $installDir $ExeName
        if (-not (Test-Path -LiteralPath $exe)) { Fail "installer finished but $exe is missing" }
        Say "installed Citadel $installed to $installDir"

        $target = Get-ShortcutTarget $startMenuLink
        if (-not $target) { Fail "Start Menu shortcut missing: $startMenuLink" }
        if ($target -ne $exe) { Warn "Start Menu shortcut points to $target, expected $exe" }
        Say "Start Menu shortcut: $startMenuLink"
        if (Get-ShortcutTarget $desktopLink) { Say "desktop shortcut: $desktopLink" } else { Warn "desktop shortcut missing: $desktopLink" }
        Say "listed under Settings > Apps as 'Citadel'; uninstall there or rerun this script with -Uninstall"
    } finally {
        $ProgressPreference = $oldProgress
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Invoke-CitadelInstaller -Version $Version -AssetsDir $AssetsDir -Uninstall:$Uninstall
