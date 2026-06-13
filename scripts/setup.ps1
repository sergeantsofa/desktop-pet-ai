# =====================================================
# Desktop Pet AI — 開發環境一鍵安裝(Windows)
# 用法:在專案根目錄以 PowerShell 執行
#   powershell -ExecutionPolicy Bypass -File scripts\setup.ps1
# =====================================================
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Test-Cmd($name) {
    return [bool](Get-Command $name -ErrorAction SilentlyContinue)
}

Write-Host "`n=== [1/5] Node.js ===" -ForegroundColor Cyan
if (Test-Cmd node) {
    Write-Host "已安裝:node $(node --version)"
} else {
    winget install --id OpenJS.NodeJS.LTS -e --accept-source-agreements --accept-package-agreements
    Write-Host "Node.js 安裝完成。若後續找不到 node,請重開 PowerShell 再執行本腳本。" -ForegroundColor Yellow
}

Write-Host "`n=== [2/5] Rust(rustup,MSVC toolchain)===" -ForegroundColor Cyan
if (Test-Cmd rustc) {
    Write-Host "已安裝:$(rustc --version)"
} else {
    winget install --id Rustlang.Rustup -e --accept-source-agreements --accept-package-agreements
    Write-Host "Rustup 安裝完成。" -ForegroundColor Yellow
}

Write-Host "`n=== [3/5] Visual Studio Build Tools(C++)===" -ForegroundColor Cyan
$vsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$hasVC = $false
if (Test-Path $vsWhere) {
    $hasVC = [bool](& $vsWhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -latest -property installationPath)
}
if ($hasVC) {
    Write-Host "已安裝 C++ Build Tools。"
} else {
    Write-Host "安裝 VS Build Tools(C++ 工作負載,需數 GB,請耐心等候)…"
    winget install --id Microsoft.VisualStudio.2022.BuildTools -e --accept-source-agreements --accept-package-agreements `
        --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive --norestart"
}

Write-Host "`n=== [4/5] npm 套件 ===" -ForegroundColor Cyan
if (Test-Cmd npm) {
    npm install --no-audit --no-fund
} else {
    Write-Host "找不到 npm(剛裝完 Node 需重開 PowerShell)。請重開後再執行本腳本。" -ForegroundColor Red
}

Write-Host "`n=== [5/5] Live2D Cubism Core ===" -ForegroundColor Cyan
$corePath = Join-Path $root "public\vendor\live2dcubismcore.min.js"
if (Test-Path $corePath) {
    Write-Host "已存在:public\vendor\live2dcubismcore.min.js"
} else {
    Write-Host "Cubism Core 為 Live2D 專有授權軟體,下載即代表你同意其授權條款:" -ForegroundColor Yellow
    Write-Host "  https://www.live2d.com/eula/live2d-proprietary-software-license-agreement_cn.html"
    $ans = Read-Host "要從 Live2D 官方 CDN 下載 live2dcubismcore.min.js 嗎?(開發用;正式發布請改用 SDK 內檔案)[y/N]"
    if ($ans -match '^[Yy]') {
        New-Item -ItemType Directory -Force -Path (Split-Path $corePath) | Out-Null
        Invoke-WebRequest -Uri "https://cubism.live2d.com/sdk-web/cubismcore/live2dcubismcore.min.js" -OutFile $corePath
        Write-Host "已下載到 public\vendor\。"
    } else {
        Write-Host "略過。請自行從 https://www.live2d.com/sdk/download/web/ 下載後放入 public\vendor\。"
    }
}

Write-Host "`n=== 完成 ===" -ForegroundColor Green
Write-Host "接下來:"
Write-Host "  1. 放置 Live2D 模型到 public\models\<角色名>\,並建立 public\models\active.json"
Write-Host "  2. (M1 對話)安裝 Ollama:winget install Ollama.Ollama;然後 ollama pull qwen2.5:7b"
Write-Host "  3. (M2 語音,選用)powershell -ExecutionPolicy Bypass -File scripts\setup-speech.ps1"
Write-Host "  4. 啟動開發模式:npm run tauri dev"
