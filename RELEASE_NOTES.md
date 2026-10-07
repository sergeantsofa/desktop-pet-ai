# Desktop Pet AI v0.2.2 — 語音辨識修好了

> **按 `Ctrl+Shift+S` 出現「找不到 whisper-cli.exe(請執行 scripts\setup-speech.ps1)」嗎?**
> 這一版修好了 —— 而且**不用再碰任何腳本**,設定裡按一下就會自己裝好。

---

## 🔧 修了什麼

### 1. 發行版根本無法使用語音輸入

舊版遇到「找不到 whisper-cli.exe」時,只叫你「執行 `scripts\setup-speech.ps1`」——
但**安裝版根本沒有那支腳本**(它在原始碼專案裡),等於語音輸入在發行版完全不能用。

現在:**設定 → 語音 → 「⬇️ 一鍵安裝(Whisper + Piper)」**,App 會自己下載安裝:

| 元件 | 用途 | 大小 |
|---|---|---|
| Whisper.cpp + `ggml-base.bin` | **語音輸入**(`Ctrl+Shift+S`) | 約 163 MB |
| Piper + 中文語音 | 高品質朗讀(可切回 Edge / 系統語音) | 約 97 MB |

- 只想用語音輸入 → 按「只裝語音輸入」(只下載 Whisper,約 163 MB)
- Whisper 模型可選 `tiny` / `base`(推薦)/ `small` / `medium`
- 有**下載進度條**,裝完會列出逐項結果,上方狀態立刻變成 `✅`
- 沒裝也能用:朗讀會退回 Edge / 系統語音

### 2. `setup-speech.ps1` 本身也是壞的

它讀 GitHub 的 `releases/latest`,但 **whisper.cpp 的正式版常常「有版號、沒有檔案」**
(例如 v1.9.5 的 assets 是空的),所以永遠找不到 Windows 套件。
已改成往回找 20 個版本、挑第一個真的帶 Windows 套件的(目前是 nightly `b5454`)。

### 3. 錯誤訊息指向不存在的檔案

現在會直接告訴你:「請到設定 → 語音按一鍵安裝」,而不是叫你去跑一個不存在的腳本。

---

## 📥 安裝

下載 `DesktopPetAI_0.2.2_x64-setup.exe` 安裝。

**語音輸入啟用步驟:**
1. 開啟 App → 托盤右鍵 → **設定** → **語音** 分頁
2. 按 **「⬇️ 一鍵安裝(Whisper + Piper)」**,等進度條跑完(約 260 MB)
3. 按 `Ctrl+Shift+S` 開始說話,再按一次結束

> 朗讀預設走 Edge 神經語音(需網路)。裝了 Piper 之後可在設定切換成完全離線的本地語音。

---

## ✅ 實測結果

- Whisper 163 MB + Piper 97 MB 全部安裝成功
- `whisper-cli.exe`、`piper.exe` 皆可正常執行(含相依 DLL)
- 完整辨識管線跑通(丟入測試音訊,成功回傳結果)
- 設定面板狀態正確更新:`Piper: ✅ zh_CN-huayan-medium.onnx / Whisper: ✅ ggml-base.bin`

## 🔐 簽章

- 公鑰 ID:`A2C4FCED4C49D98F`
- v0.1.x 內嵌舊公鑰,無法自動更新 → 需手動裝一次;v0.2.x 之後即可自動更新

## ⚠️ 提醒

- 這一版沒有包含 Cubism Core / 角色模型?**有的**,從 v0.2.1 起已納入版控,
  裝完開啟就會看到角色(官方範例 Haru)。
- 想用自己的模型:設定 → 關於・更新 → 開啟資料夾,放進 `models\`。

---

*完整改動記錄見 [`0615技術歸檔.md`](0615技術歸檔.md)。*
