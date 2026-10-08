# Installs Kern on Windows from a GitHub release, checked against the release's SHA256SUMS. In PowerShell:
#   irm https://github.com/OWNER/kern/releases/latest/download/install.ps1 | iex
# $env:KERN_VERSION = 'v0.4.0' picks the release, $env:KERN_INSTALL_DIR the folder (default %LOCALAPPDATA%\Programs\kern).
# Works in Windows PowerShell 5.1 and PowerShell 7. The release workflow writes its repository and tag into this file.
& {
    $ErrorActionPreference = 'Stop'
    # The progress bar makes downloads in Windows PowerShell many times slower.
    $ProgressPreference = 'SilentlyContinue'
    $repo = if ($env:KERN_REPO) { $env:KERN_REPO } else { '__KERN_REPO__' }
    $version = if ($env:KERN_VERSION) { $env:KERN_VERSION } else { '__KERN_VERSION__' }
    if ("$repo$version" -like '*__KERN_*') { throw 'kern install: this copy of install.ps1 is not from a release; set $env:KERN_REPO and $env:KERN_VERSION' }
    $base = if ($env:KERN_DOWNLOAD_URL) { $env:KERN_DOWNLOAD_URL.TrimEnd('/') } else { "https://github.com/$repo/releases/download/$version" }
    $dir = if ($env:KERN_INSTALL_DIR) { $env:KERN_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\kern' }
    # Windows 11 on Arm runs the x64 build through its built-in emulation.
    $asset = 'kern-windows-x64.exe'
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ('kern-' + [Guid]::NewGuid())
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $file = Join-Path $tmp $asset
        $sums = Join-Path $tmp 'SHA256SUMS'
        try { Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile $file } catch { throw "kern install: could not download $base/$asset ($($_.Exception.Message))" }
        try { Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile $sums } catch { throw "kern install: could not download $base/SHA256SUMS ($($_.Exception.Message))" }
        $want = $null
        foreach ($line in Get-Content $sums) {
            $parts = $line.Trim() -split '\s+', 2
            if ($parts.Count -eq 2 -and $parts[1].TrimStart('*') -eq $asset) { $want = $parts[0].ToLowerInvariant(); break }
        }
        if (-not $want) { throw "kern install: SHA256SUMS of $version does not list $asset; nothing was installed" }
        $got = (Get-FileHash -Algorithm SHA256 -Path $file).Hash.ToLowerInvariant()
        if ($got -ne $want) { throw "kern install: the download does not match SHA256SUMS (expected $want, got $got); nothing was installed" }

        # Tried where it will live, then moved into place in one step, so a failed install never leaves a broken kern behind.
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        # Windows runs only files that end in .exe, so the trial copy is kern-new.exe.
        $new = Join-Path $dir 'kern-new.exe'
        Copy-Item -Force -Path $file -Destination $new
        try {
            $runs = $false
            try { & $new --version | Out-Null; $runs = ($LASTEXITCODE -eq 0) } catch { $runs = $false }
            if (-not $runs) { throw 'kern install: the downloaded Kern does not run on this machine; nothing was installed' }
            try { Move-Item -Force -Path $new -Destination (Join-Path $dir 'kern.exe') } catch { throw "kern install: cannot replace $dir\kern.exe; close any running kern and try again" }
        } finally {
            if (Test-Path -LiteralPath $new) { Remove-Item -Force -LiteralPath $new }
        }
    } finally {
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    }

    # The user's PATH, read and written as stored, so entries such as %USERPROFILE%\bin stay as they are.
    $key = Get-Item -Path 'HKCU:\Environment'
    $raw = [string]$key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
    $present = $raw -split ';' | Where-Object { $_ -and ([Environment]::ExpandEnvironmentVariables($_).TrimEnd('\') -ieq $dir.TrimEnd('\')) }
    if (-not $present) {
        $kind = if ($raw -like '*%*') { 'ExpandString' } else { 'String' }
        Set-ItemProperty -Path 'HKCU:\Environment' -Name 'Path' -Value ((@($raw.TrimEnd(';'), $dir) | Where-Object { $_ }) -join ';') -Type $kind
        # Setting a user variable through .NET tells Windows the environment changed, so new terminals see the new PATH.
        [Environment]::SetEnvironmentVariable('KERN_INSTALL_REFRESH', '1', 'User')
        [Environment]::SetEnvironmentVariable('KERN_INSTALL_REFRESH', $null, 'User')
        $env:Path = "$env:Path;$dir"
        Write-Host "Added $dir to your PATH; new terminals will find kern."
    }
    Write-Host "Installed Kern $version to $dir\kern.exe"
}
