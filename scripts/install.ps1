param(
    [string]$Version = "latest",
    [string]$InstallDir = "$env:ProgramFiles\SupplyDrop\bin",
    [string]$BinName = "supply-drop-ssh.exe",
    [string]$Repo = "Mesh-America/supply-drop-ssh-transport-plugin",
    [string]$BbsBin = "supply-drop-bbs",
    [bool]$RegisterPlugin = $true
)

$ErrorActionPreference = "Stop"
$MinBbsVersion = [version]"0.6.0"

function Fail($Message) {
    Write-Error $Message
    exit 1
}

function Get-BbsVersion($Command) {
    $versionOutput = & $Command --version 2>$null
    $match = [regex]::Match(($versionOutput -join "`n"), '\d+\.\d+\.\d+')
    if (-not $match.Success) {
        Fail "Could not detect Supply Drop BBS version from '$Command --version'"
    }
    return [version]$match.Value
}

function Register-SshPlugin($Command, $PluginPath) {
    if (-not $RegisterPlugin) {
        Write-Host "Skipping Supply Drop plugin registration because RegisterPlugin is false"
        return
    }

    if (-not (Get-Command $Command -ErrorAction SilentlyContinue)) {
        Fail "Missing $Command. Install or upgrade Supply Drop BBS v$MinBbsVersion+ before registering the plugin, or rerun with -RegisterPlugin:`$false to install only the binary."
    }

    $bbsVersion = Get-BbsVersion $Command
    if ($bbsVersion -lt $MinBbsVersion) {
        Fail "Supply Drop BBS v$MinBbsVersion+ is required for plugins.d registration; found v$bbsVersion"
    }

    Write-Host "Registering SSH process plugin with Supply Drop BBS v$bbsVersion..."
    & $Command plugin add ssh $PluginPath
}

$asset = switch ($env:PROCESSOR_ARCHITECTURE) {
    "AMD64" { "windows-x86_64" }
    "x86" { Fail "32-bit Windows is not supported" }
    "ARM64" { Fail "Windows ARM64 release packages are not published yet" }
    default { Fail "Unsupported Windows architecture: $env:PROCESSOR_ARCHITECTURE" }
}

$archive = "supply-drop-ssh-transport-plugin-$asset.zip"
if ($Version -eq "latest") {
    $url = "https://github.com/$Repo/releases/latest/download/$archive"
    $tag = $null
} else {
    $tag = if ($Version.StartsWith("v")) { $Version } else { "v$Version" }
    $url = "https://github.com/$Repo/releases/download/$tag/$archive"
}

$temp = Join-Path ([System.IO.Path]::GetTempPath()) "supply-drop-ssh-$([System.Guid]::NewGuid())"
New-Item -ItemType Directory -Path $temp | Out-Null

try {
    $zip = Join-Path $temp "plugin.zip"
    $unpacked = Join-Path $temp "unpacked"
    $headers = @{}
    if ($env:GITHUB_TOKEN) {
        $headers["Authorization"] = "Bearer $env:GITHUB_TOKEN"
    }

    Write-Host "Downloading $archive from $Version..."
    try {
        Invoke-WebRequest -Uri $url -OutFile $zip -Headers $headers
    } catch {
        if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
            throw
        }

        Write-Host "Direct download failed; trying GitHub CLI..."
        $releaseTag = $tag
        if (-not $releaseTag) {
            $releaseTag = gh release view --repo $Repo --json tagName -q .tagName
        }
        gh release download $releaseTag --repo $Repo --pattern $archive --dir $temp
        $downloaded = Join-Path $temp $archive
        if (-not (Test-Path $downloaded)) {
            Fail "GitHub CLI did not download $archive"
        }
        Move-Item -Path $downloaded -Destination $zip -Force
    }
    Expand-Archive -Path $zip -DestinationPath $unpacked -Force

    $source = Get-ChildItem -Path $unpacked -Recurse -Filter "supply-drop-ssh.exe" |
        Select-Object -First 1
    if (-not $source) {
        Fail "Archive did not contain supply-drop-ssh.exe"
    }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $dest = Join-Path $InstallDir $BinName
    Copy-Item -Path $source.FullName -Destination $dest -Force
    Register-SshPlugin -Command $BbsBin -PluginPath $dest

    Write-Host "Installed $BinName to $dest"
    Write-Host ""
    Write-Host "SSH plugin registration complete. Restart Supply Drop BBS to activate it."
    Write-Host ""
    Write-Host "The plugin listens on port 2222 and generates a persistent SSH host key"
    Write-Host "on first start. Connect with:"
    Write-Host "  ssh -p 2222 <bbs-host>"
} finally {
    Remove-Item -Recurse -Force $temp -ErrorAction SilentlyContinue
}
