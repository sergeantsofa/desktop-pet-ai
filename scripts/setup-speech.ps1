# =====================================================
# Desktop Pet AI — 語音 sidecar 一鍵安裝(M2)
#   Piper TTS(高品質中文語音 + 真實對嘴)
#   Whisper.cpp STT(Ctrl+Shift+S 語音輸入)
# 用法:
#   powershell -ExecutionPolicy Bypass -File scripts\setup-speech.ps1
#   參數 -WhisperModel base|small|medium(預設 base;越大越準、越慢)
# 安裝目錄:%APPDATA%\com.desktoppet.ai\speech\
# =====================================================
param(
    [ValidateSet("tiny", "base", "small", "medium")]
    [string]$WhisperModel = "base"
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"  # Invoke-WebRequest 大檔下載加速

$speechDir = Join-Path $env:APPDATA "com.desktoppet.ai\speech"
$piperDir = Join-Path $speechDir "piper"
$whisperDir = Join-Path $speechDir "whisper"
New-Item -ItemType Directory -Force -Path $piperDir, $whisperDir | Out-Null

function Get-LatestAsset($repo, $namePattern) {
    $rel = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest"
    return $rel.assets | Where-Object name -match $namePattern | Select-Object -First 1
}

# 用 Windows 內建 curl.exe 下載(PS 5.1 的 Invoke-WebRequest 對 HuggingFace 轉址會出錯)
function Download($url, $outFile) {
    & curl.exe -L --fail --retry 3 -o $outFile $url
    if ($LASTEXITCODE -ne 0) { throw "下載失敗:$url" }
}

function Install-FromZip($url, $exeName, $destDir) {
    $tmpZip = Join-Path $env:TEMP "pet-speech-dl.zip"
    $tmpDir = Join-Path $env:TEMP "pet-speech-dl"
    Download $url $tmpZip
    if (Test-Path $tmpDir) { Remove-Item -Recurse -Force $tmpDir }
    Expand-Archive -Path $tmpZip -DestinationPath $tmpDir
    # 找到主執行檔所在資料夾,整夾搬過去(含 dll / espeak-ng-data 等)
    $exe = Get-ChildItem -Recurse -Path $tmpDir -Filter $exeName | Select-Object -First 1
    if (-not $exe) { throw "壓縮檔內找不到 $exeName" }
    Copy-Item -Path (Join-Path $exe.DirectoryName "*") -Destination $destDir -Recurse -Force
    Remove-Item -Force $tmpZip
    Remove-Item -Recurse -Force $tmpDir
}

Write-Host "`n=== [1/4] Piper TTS ===" -ForegroundColor Cyan
if (Test-Path (Join-Path $piperDir "piper.exe")) {
    Write-Host "已安裝:piper.exe"
} else {
    $asset = Get-LatestAsset "rhasspy/piper" "windows_amd64.*\.zip$"
    if (-not $asset) { throw "在 rhasspy/piper releases 找不到 Windows 套件,請手動下載放入 $piperDir" }
    Write-Host "下載 $($asset.name)…"
    Install-FromZip $asset.browser_download_url "piper.exe" $piperDir
    Write-Host "完成。"
}

Write-Host "`n=== [2/4] Piper 中文語音模型 ===" -ForegroundColor Cyan
if (Get-ChildItem $piperDir -Filter "*.onnx" -ErrorAction SilentlyContinue) {
    Write-Host "已存在語音模型(.onnx)"
} else {
    # Piper 官方中文語音(中國普通話;Hugging Face rhasspy/piper-voices)
    $voiceBase = "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/zh/zh_CN/huayan/medium"
    Write-Host "下載 zh_CN-huayan-medium…"
    Download "$voiceBase/zh_CN-huayan-medium.onnx" (Join-Path $piperDir "zh_CN-huayan-medium.onnx")
    Download "$voiceBase/zh_CN-huayan-medium.onnx.json" (Join-Path $piperDir "zh_CN-huayan-medium.onnx.json")
    Write-Host "完成。想換語音可自行放其他 .onnx(+.onnx.json)進 $piperDir"
}

Write-Host "`n=== [3/4] Whisper.cpp ===" -ForegroundColor Cyan
$hasWhisper = (Test-Path (Join-Path $whisperDir "whisper-cli.exe")) -or (Test-Path (Join-Path $whisperDir "main.exe"))
if ($hasWhisper) {
    Write-Host "已安裝:whisper-cli.exe / main.exe"
} else {
    $asset = Get-LatestAsset "ggml-org/whisper.cpp" "bin-x64\.zip$"
    if (-not $asset) { throw "在 ggml-org/whisper.cpp releases 找不到 Windows 套件,請手動下載放入 $whisperDir" }
    Write-Host "下載 $($asset.name)…"
    try {
        Install-FromZip $asset.browser_download_url "whisper-cli.exe" $whisperDir
    } catch {
        Install-FromZip $asset.browser_download_url "main.exe" $whisperDir
    }
    Write-Host "完成。"
}

Write-Host "`n=== [4/4] Whisper 模型(ggml-$WhisperModel)===" -ForegroundColor Cyan
$modelPath = Join-Path $whisperDir "ggml-$WhisperModel.bin"
if (Get-ChildItem $whisperDir -Filter "ggml-*.bin" -ErrorAction SilentlyContinue) {
    Write-Host "已存在 Whisper 模型(ggml-*.bin)"
} else {
    Write-Host "下載 ggml-$WhisperModel.bin(base 約 148MB,請稍候)…"
    Download "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-$WhisperModel.bin" $modelPath
    Write-Host "完成。"
}

Write-Host "`n=== 完成 ===" -ForegroundColor Green
Write-Host "重新啟動 App 後:"
Write-Host "  - AI 回覆會用 Piper 語音朗讀並對嘴(設定面板可切回系統語音)"
Write-Host "  - Ctrl+Shift+S 開始/結束語音輸入"
