<#
  Build the release DLL and strip build-machine paths from it.

  Rust embeds absolute source paths in the binary (panic locations) and the MSVC
  linker records the full PDB path in the PE debug directory. Shipping such a DLL
  leaks C:\Users\<name>\... to everyone who downloads it, so this script:
    1. remaps the user profile prefix to C:\build
    2. sets /PDBALTPATH so only the PDB file name is recorded
    3. verifies the result contains no user name before you publish it

  Usage:
    powershell -ExecutionPolicy Bypass -File tools\build.ps1
#>
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Push-Location $root

try {
    if (-not $env:LIB) {
        $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
        if (Test-Path $vswhere) {
            $vsPath = & $vswhere -latest -products * -property installationPath
            if ($vsPath) {
                $mvc = Get-ChildItem "$vsPath\VC\Tools\MSVC" -Directory | Sort-Object Name | Select-Object -Last 1
                if ($mvc) {
                    $env:LIB = "$($mvc.FullName)\lib\x64;" + $env:LIB
                    Write-Host "MSVC: $($mvc.FullName)"
                }
            }
        }
    }

    $remap = '--remap-path-prefix="' + $env:USERPROFILE + '"=C:\build'
    $flags = @("-C", "link-arg=/PDBALTPATH:astra_openni.pdb", $remap)
    if ($env:RUSTFLAGS) { $flags = @($env:RUSTFLAGS) + $flags }
    $env:RUSTFLAGS = $flags -join " "

    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

    $dll = Join-Path $root "target\release\astra_openni.dll"

    # std is precompiled with the toolchain's own paths baked in, so
    # --remap-path-prefix cannot reach those strings. Replace them in place with
    # an equal-length placeholder (same length keeps all offsets valid).
    $bytes = [System.IO.File]::ReadAllBytes($dll)
    $needle = [System.Text.Encoding]::ASCII.GetBytes($env:USERPROFILE)
    $filler = "C:\build\" + ("x" * ($needle.Length - 8))
    $repl = [System.Text.Encoding]::ASCII.GetBytes($filler)

    $hits = 0
    $i = 0
    $last = $bytes.Length - $needle.Length
    while ($i -le $last) {
        $match = $true
        for ($j = 0; $j -lt $needle.Length; $j++) {
            if ($bytes[$i + $j] -ne $needle[$j]) { $match = $false; break }
        }
        if ($match) {
            [Array]::Copy($repl, 0, $bytes, $i, $repl.Length)
            $hits++
            $i += $needle.Length
        }
        else {
            $i++
        }
    }
    if ($hits -gt 0) {
        [System.IO.File]::WriteAllBytes($dll, $bytes)
        Write-Host "replaced $hits embedded path occurrence(s)"
    }

    $text = [System.Text.Encoding]::ASCII.GetString([System.IO.File]::ReadAllBytes($dll))
    Write-Host ("DLL: {0} ({1} bytes)" -f $dll, (Get-Item $dll).Length)
    if ($text.Contains($env:USERNAME) -or $text.Contains($env:USERPROFILE)) {
        Write-Warning "DLL still contains this machine's user name - do not publish it"
    }
    else {
        Write-Host "path sanitisation check passed"
    }
}
finally {
    Pop-Location
}
