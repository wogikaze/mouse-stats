# 配布と運用

## Windows

`npm ci` → CIと同じ検査 → `npm run desktop -- build` でNSISインストーラーを生成します。バージョンはdesktopのpackage.json、Cargo.toml、tauri.conf.jsonを同時に更新します。

データ保存先はTauriのapp_data_dir（通常 `%APPDATA%/dev.wgkz.mouse-stats`）。`statistics.db`、WAL/SHM、DPAPI暗号化した`session.dpapi`があります。ログイン直後や同期で通信が切れても集計はローカルに残ります。アプリ終了時はflushします。異常終了は通常最大5秒の未保存分を失う可能性があります。

バックアップはトレイから終了後にDBと関連ファイルをコピーするか、SQLiteのバックアップAPIを使用します。稼働中のDBだけをコピーしないでください。DPAPIは同じWindowsユーザー/端末に依存し、別PCでのトークン復号はできません。DBを複数PCへクローンして同時使用しないでください。

Authenticode署名は組織のコード署名証明書/Trusted Signingで実施します。Tauriの `bundle.windows.signCommand` に環境固有の署名コマンドを設定し、署名鍵をリポジトリに保存しません。生成後 `Get-AuthenticodeSignature` で検証します。証明書が未提供の環境では署名済みとは扱いません。

自動更新を配布する場合はTauri updaterの署名鍵をCI secretで用意し、HTTPS更新メタデータと公開鍵を設定します。未署名の更新を許可する設定はしません。現時点のインストーラーは手動更新用です。アプリ更新前にDBをバックアップし、スキーマを戻す際は互換性を検証します。

署名済み自動更新を有効にするリリースでは、次の手順で準備します。現在のMVPビルドに更新プラグインや更新フィードはまだ含めていません。

1. Tauri CLIのsigner generateで更新署名鍵を生成し、秘密鍵をCIのTAURI_SIGNING_PRIVATE_KEY、パスフレーズをTAURI_SIGNING_PRIVATE_KEY_PASSWORDへ登録します。生成した秘密鍵ファイルをGitへ追加しません。
2. tauri-plugin-updaterを導入・登録し、公開鍵と固定HTTPS更新URLをtauri.conf.jsonのplugins.updaterへ設定します。bundle.createUpdaterArtifactsを有効にします。
3. CIで同じバージョンのインストーラーと署名を生成し、更新メタデータのversion・pub_date・platforms.windows-x86_64のurlとsignatureを設定します。署名検証が必要なため、開発用の未署名バイナリを更新先にしません。
4. 前の署名済み版をテスト環境に入れ、更新チェック・署名改ざん拒否・ダウンロード失敗・更新後のDB migrationと起動を検証します。フィード公開前にこれらの証拠を受け入れ台帳へ追加します。

## Cloudflare / Void

1. PowerShellで環境変数MOUSE_STATS_ORIGINに固定のHTTPS URLを設定します。ORIGINとRP_IDはこのURLから生成されます。URLを省略したdeployは拒否されます。PasskeyはRP_IDへ固定されるため、登録後の変更は避けます。
2. `npm run db:generate -w @mouse-stats/cloud` でmigrationを生成し、差分をレビュー。`npm run db:migrate -w @mouse-stats/cloud` でローカル適用します。
3. Cloudflareへログイン後、`npm run deploy -w @mouse-stats/cloud` を実行。VoidがD1を作成・migration適用・Worker配備します。生成されたvoid.lock.jsonをコード管理します。トークンはCI secretへ置きます。
4. HTTPSのhealth、Passkey登録/ログイン、端末接続、同期、失効、世代変更を再確認します。ローカル検証だけで本番稼働済みとは扱いません。

GitHubのproduction環境へCLOUDFLARE_ACCOUNT_IDとCLOUDFLARE_API_TOKENを登録し、Deploy Cloud workflowをorigin指定で手動実行できます。生成されたresource-lock artifactを確認し、void.lock.jsonを保存します。PRから本番DBへ書き込みません。

ロールバックはVoidのdeployment履歴から前のWorkerへ戻します（`void deploy --help` / Void Deployment docsで対象バージョンのコマンドを確認）。D1 migrationは自動で巻き戻りません。破壊的migrationの前にD1 exportまたはTime Travelの復元地点を確保し、WorkerとDBの互換性を保ちます。

Void 1.1.12では、cloudディレクトリから `void project rollback <Worker-Version-ID> --platform cloudflare` を使います。DBも戻す場合はdeployが出力したbookmarkを使い、`void db restore --bookmark <bookmark> --platform cloudflare` を実行します。復元地点より後の書き込みは失われるため、対象DB・復元地点・アプリの互換性を確認してから実行します。WorkerとDBは別々に復元します。

障害時はUIの最終保存・最終同期・エラーを確認します。ネット断ではローカル保存を継続し、再送時はMAX UPSERTで二重計上しません。生キー・トークン・Passkey秘密情報・復旧コードは障害ログへ載せません。D1使用量はWorkers/D1ダッシュボードで監視し、429や上限時はリトライさせます。毎回の同期量は20行/バッチです。

## 負荷測定

Windows実機で30分以上トレイ常駐、1000Hzマウス、複数マウス、長押し、ロック/復帰を組み合わせます。ProcessのCPU時間増分、WorkingSet、DBファイル増分、送信バイトを記録します。合成イベント単体テストの所要時間を実機Raw Inputの欠測率の証明には使いません。

アプリを起動して計測状態を確認し、画面を閉じた後に次の読み取り専用スクリプトを実行します。アプリの設定や同意状態は変更しません。30分間のサンプルをJSONに保存します。

```powershell
powershell -NoProfile -File scripts/measure-native.ps1 -DurationSeconds 1800 -IntervalSeconds 5 -OutputPath test-results/native-resources-30min.json
```

Alternatively, run `npm run test:soak` to launch the release app hidden for 30 minutes and measure its process tree. The harness verifies that saved consent is enabled, measurement is not paused, cloud sync is disabled, the window is hidden, and the input listener is ready; it aborts without changing settings if any condition fails, and exits the app cleanly after the sample. Pass `--duration-seconds 2` for a short harness check. It does not synthesize physical input or claim an event-loss measurement.

統計UIはウィンドウにフォーカスがある間だけ5秒ごとにsnapshotを読み直します。フォーカスを失うと更新を止め、ウィンドウが非表示ならチャートをアンマウントします。Raw Inputの計測はUIの更新タイマーとは独立して動作します。

対象が複数ある場合は `-ProcessId <PID>` で選びます。データ保存先を変更した環境では `-DataDirectory <実際のフォルダー>` も指定します。対象はmouse-stats.exeに限定し、WebView2を含む子プロセスも集計します。途中で対象プロセスが終了した場合は `end_reason=process_exited` を保存し、終了コード2で返します。

結果にはCPU時間と平均使用率（1コアを100%とする値・全論理CPUを100%とする値の両方）、サンプル中の最大WorkingSet/Private Bytes、I/O転送量、DB/WAL/SHMの合計容量増分が含まれます。各サンプルには対象プロセスツリーのPID・プロセス名・WorkingSet・Private Bytesも含み、集計ピークの発生元を確認できます。カウンターの意味は[Win32_Processの公式仕様](https://learn.microsoft.com/windows/win32/cimwin32prov/win32-process)に従います。WorkingSetの共有ページは重複し得ます。I/Oはディスクへの実書込量やネットワーク送信量とは異なり、後者は別途測定します。サンプル間で終了した子プロセスと、初回観測前の処理は集計できないため、結果は観測した増分です。

レポートと一緒に、マウスの設定Hz、接続台数、試用した操作、ロック/復帰の有無、計測・同期の設定を記録します。スクリプトは物理入力・計測状態・画面の非表示・欠測率を判定しません。これらと30分間の継続計測は実機で確認します。`npm run test:native` は配布exeへ2秒間接続して測定処理自体を検証し、`test-results/native-resources-smoke.json` を生成します。この短時間テストを実機負荷試験の代用にはしません。
