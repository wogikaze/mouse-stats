# Mouse Stats

Windows 10/11用の入力統計アプリ。Tauri 2 / Rust / Vue 3。Raw Inputの相対移動量と物理キーごとの初回押下回数だけをUTCの時間単位で保存します。入力文字・イベント順序・使用アプリは保存しません。

## 開発

Windows、MSVC C++ Build Tools、WebView2、Rust stable、Node **24.22以上**が必要です（Voidの要件）。システムのNodeが古い場合も、`npm install`後のnpm scriptsは同梱のNode 24を使用します。

```powershell
npm ci
npm run desktop -- dev
```

UIのみ: `npm run dev`。ブラウザ表示では入力計測やDBへのアクセスはできません。クラウド: `npm run dev:cloud` → **http://localhost:5173** （Passkeyの開発にはIPアドレスではなくlocalhostを使います）。クラウドはローカルD1で動き、初回起動時にmigrationを適用します。

初回起動ではプライバシー説明に同意してから計測開始。画面を閉じても常駐し、トレイから再表示・停止・終了できます。自動起動とクラウド同期は初期状態ではOFFです。

## 検証・ビルド

```powershell
npm run check
npm test
npm run build
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
node_modules/.bin/playwright.cmd test --config apps/cloud/playwright.config.ts
 npm run test:ui
 npm run test:native
npm run desktop -- build
```

インストーラー: `target/release/bundle/nsis/`。テストデータは本番データと別で生成します。ブラウザE2Eは仮想認証器を使用し、Windows Helloの対話操作は不要です。

常駐中の負荷は `powershell -NoProfile -File scripts/measure-native.ps1` で30分間測定し、`test-results/native-resources.json`へ保存できます。実機試用の手順と測定の範囲は[運用ドキュメント](docs/OPERATIONS.md#負荷測定)を参照してください。

## 同期

設定からHTTPSのクラウドURLを保存し、ブラウザのユーザー名＋Passkeyで接続します。初回接続前に過去のローカル統計をアップロードするか選べます。同期ON後、起動時・手動・原則5分ごとに同期し、失敗時はバックオフします。開発ビルドのみlocalhostのHTTPを許可します。

各プロセスのUUIDとUTC時間枠を一意キーにし、クラウドは累計値のMAXで更新。ローカルACKは送信したrevisionに対してのみ適用します。現在のデバイスはローカルとクラウド復元をMAXマージし、別PCはキャッシュに保存します。

Passkey・デバイス・復旧コード・クラウド統計削除・アカウント削除はブラウザのアカウント管理で再認証して操作します。ローカル削除はクラウド削除とは独立しています。アカウントの統計削除後、旧世代の記録はサーバーで拒否します。

データ形式・運用: [API](docs/API.md)、[運用・配布](docs/OPERATIONS.md)、[受け入れ検証](docs/ACCEPTANCE.md)。元の全仕様は [PLAN.md](PLAN.md) にあります。
