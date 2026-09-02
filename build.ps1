$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true

$env:RUSTC_BOOTSTRAP = 1
$config = "./.cargo/release.toml"

if (Get-Command "msrustup" -ErrorAction SilentlyContinue) {
    # The default C2/MSVC toolchain cannot compile this project.
    $env:MSRUSTUP_TOOLCHAIN = "ms-prod@llvm"
    $config = "./.cargo/release-windows-ms.toml"
}

# Extract the package version from Cargo.toml so we can stamp it into the installer.
$cargoToml = Get-Content -Raw -LiteralPath "Cargo.toml"
$versionMatch = [regex]::Match($cargoToml, '(?m)^version\s*=\s*"([^"]+)"')
if (!$versionMatch.Success) {
    throw "Failed to extract version from Cargo.toml"
}
$version = $versionMatch.Groups[1].Value

cargo build --config $config --release --target aarch64-pc-windows-msvc
cargo build --config $config --release --target x86_64-pc-windows-msvc

$iscc = "C:\Program Files\Inno Setup 7\ISCC.exe"
if (!(Test-Path $iscc)) {
    $iscc = "$env:LocalAppData\Programs\Inno Setup 7\ISCC.exe"
    if (!(Test-Path $iscc)) {
        throw "Please install Inno Setup 7: https://jrsoftware.org/isdl.php"
    }
}

& $iscc /DAppVersion=$version /DArchitecturesAllowed=arm64 /DSource=target\aarch64-pc-windows-msvc\release\coreutils.exe /O. /Fcoreutils-arm64 coreutils.iss
& $iscc /DAppVersion=$version /DArchitecturesAllowed=x64os /DSource=target\x86_64-pc-windows-msvc\release\coreutils.exe  /O. /Fcoreutils-x64 coreutils.iss
