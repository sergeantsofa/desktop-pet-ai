# Live2D 模型放置說明

基於授權考量,本專案**不附帶任何 Live2D 模型**,請放入你自有或已取得授權的模型。

## 步驟

1. 將整個模型資料夾複製到這裡,例如:

   ```
   public/models/Hiyori/
   ├── Hiyori.model3.json
   ├── Hiyori.moc3
   ├── textures/...
   ├── motions/...
   └── ...
   ```

2. 在 `public/models/` 建立 `active.json`(可複製 `active.example.json` 修改):

   ```json
   {
     "path": "/models/Hiyori/Hiyori.model3.json",
     "scale": 1.0,
     "idleMinutes": 3
   }
   ```

3. 重新啟動 App。

## 注意

- 目前支援 Cubism 3 / 4 模型(`.model3.json`)。
- 免費範例模型可至 Live2D 官網下載(遵守 Free Material License)。
- `active.json` 與模型檔皆已被 .gitignore 排除,不會進版控。
