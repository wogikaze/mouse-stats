# Windows 入力統計アプリ 開発計画（PLAN.md）

- ステータス: MVP 実装・受け入れ検証中（元の仕様範囲は維持。実装と検証結果は [受け入れ台帳](docs/ACCEPTANCE.md) を参照）
- 更新日: 2026-10-20
- 対象 OS: Windows 10/11（Windows 11 を優先して検証）
- 目的: マウスの移動量と物理キー別の押下回数を、常時計測・可視化・任意のクラウド同期によって確認できるアプリを作る。

## 1. プロダクト方針

### 1.1 MVP で達成すること

1. Windows ログイン後、設定に従ってバックグラウンドで計測する。統計画面を閉じても継続する。
2. マウス相対移動量と物理キーごとの押下回数を、UTC の **1 時間単位**でローカル SQLite に保存する。
3. 「今日・7 日・30 日・全期間・任意期間」の統計、細い年間カレンダーヒートマップ、時間帯別統計、物理キーボードヒートマップを表示する。
4. アカウント不要・オフラインで使用できる。明示的な操作でクラウド同期を有効にできる。
5. クラウド利用時は **ユーザー名 + Passkey** でログインし、複数 Windows PC の統計を同じアカウントから参照できる。
6. 同期は冪等にし、再送・通信断・複数端末・アプリ再起動による二重計上を防ぐ。
7. 計測の一時停止、デバイス失効、データ削除、同期状態の確認ができる。

### 1.2 MVP でやらないこと

- 入力した文字列、押下順序、アクティブウィンドウ名、アプリ名、クリップボードの記録。
- 押下キー列やマウスの生イベントをクラウドへ送信すること。
- 実際に操作していない時間帯の入力推定。
- スマートフォンや macOS/Linux の入力計測。
- マウスの物理距離（cm / m）の**自動**判定。CPI を知らなければ raw counts として扱う。
- ランキング、フレンド機能、公開プロフィール。

## 2. 採用技術

| 領域 | 技術 | 補足 |
| --- | --- | --- |
| デスクトップ | Tauri 2 | トレイ、常駐、自動起動、単一インスタンス、Deep Link |
| Windows 入力取得 | Rust + Win32 Raw Input | 非表示 HWND と独立メッセージループ |
| デスクトップ UI | Vue 3 + TypeScript | UI は計測処理に依存しない |
| ビルド/開発ツールチェーン | Vite+ 1.0 | Vite / Rolldown / Oxc / Oxlint / Oxfmt / Vitest / Vite Task |
| スタイル | Tailwind CSS 4 | ダークテーマ優先 |
| チャート | ECharts | カレンダー・時間帯・ランキング。キーボードは Vue/SVG |
| ローカル DB | SQLite + Rust `rusqlite` | オフライン・集計値・同期状態 |
| クラウド API | Void（内部 Hono）+ Cloudflare Workers | Void のファイルベースルートを利用 |
| クラウド DB | Cloudflare D1 + Drizzle（Void） | migrations をコード管理 |
| 認証 | WebAuthn / Passkeys（SimpleWebAuthn を候補） | ブラウザ経由。メール・パスワードは不要 |
| CI | GitHub Actions | Windows ビルド、Rust/TS の検査、デプロイ |

**方針:** VoidZero 系のツールは Web/TypeScript 側に積極採用し、Windows ネイティブ処理は Rust に分ける。新しい Void に不具合がある場合でも、API 契約を維持して通常の Hono + Workers に差し替えられること。

参考: [Vite+](https://viteplus.dev/guide/)、[Void Quickstart](https://void.cloud/guide/quickstart)、[Void Server Routing](https://void.cloud/guide/server-routing)、[Void D1](https://void.cloud/guide/database/d1)、[Tauri 2](https://v2.tauri.app/)、[Windows Raw Input](https://learn.microsoft.com/windows/win32/inputdev/raw-input)。

## 3. システム構成

```text
Windows PC
  Tauri 2
    Rust / Raw Input  ──> Aggregator ──> SQLite (5 秒ごと)
       │                                  │
       └── Tray / Start / Stop            ├── Vue UI（ローカル統計）
                                          └── Sync Worker（5 分ごと、同期 ON のみ）
                                                       │ HTTPS
                                                       ▼
Cloudflare Workers / Void ──> D1
  API 認証・デバイス検証         ユーザー・Passkey・端末・計測セッション・時間集計
         │
         └── HTTPS WebAuthn ログインページ（システムブラウザ）
```

### 3.1 リポジトリ構造（想定）

```text
/
├── PLAN.md
├── apps/
│   ├── desktop/
│   │   ├── src/                 # Vue UI、各ビュー、状態管理
│   │   └── src-tauri/
│   │       └── src/
│   │           ├── input/       # Win32 Raw Input、キー正規化
│   │           ├── aggregate/   # 時間単位の集計
│   │           ├── db/          # SQLite、migration
│   │           ├── sync/        # アップロード・ダウンロード
│   │           └── app/         # Tray / lifecycle / OS イベント
│   └── cloud/
│       ├── routes/api/v1/       # Void の HTTP API
│       ├── pages/               # ブラウザ認証・アカウント管理
│       ├── db/schema.ts         # Drizzle の D1 schema
│       └── void.config.ts
├── packages/
│   └── contracts/               # API 型、集計フォーマット、キー ID 定義
└── .github/workflows/
```

ルート・生成コマンド・Vue の統合手順は実際の Vite+/Void のテンプレートで検証してから確定する。`packages/contracts` は TS の API スキーマを中心にし、Rust 側の型はコード生成または対応するテストで一致を保証する。

## 4. Windows 常時計測の動作

### 4.1 ライフサイクル

| イベント | 動作 |
| --- | --- |
| 初回起動 | ローカル SQLite を作り、プライバシー説明後に計測開始。クラウドは OFF |
| Windows ログイン | 自動起動が ON ならバックグラウンド起動 |
| ウィンドウ閉じる | ウィンドウのみ隠し、計測と同期を継続 |
| トレイ「開く」 | 統計画面を表示 |
| トレイ「一時停止」 | 計測値を加算しない。同期は継続 |
| トレイ「再開」 | 以後の入力のみ計測 |
| トレイ「終了」 | SQLite に flush してアプリ終了。ネットワーク応答待ちは期限付き |
| スリープ/ロック | OS が配信しなかったイベントは補完しない |
| 復帰 | 受信を継続し、必要に応じて同期を再試行 |
| Windows 二重起動 | 既存プロセスへ表示要求を送る。計測プロセスは 1 つだけ |
| 異常終了・強制終了 | 最後に保存できた集計値まで復旧（通常最大約 5 秒の未保存分は失われ得る） |

### 4.2 Raw Input

- Rust 専用スレッドに非表示 HWND と Windows メッセージループを持つ。`RegisterRawInputDevices` の `RIDEV_INPUTSINK` を使用する。
- `WM_INPUT` から `GetRawInputData` でマウス/キーボードイベントを取得する。`RIDEV_NOLEGACY` は指定しない。
- Vue や WebView の再描画・表示状態に計測を依存させない。
- 入力イベントはメモリ上で集計し、DB ライタースレッドへバッチで渡す。高頻度入力中にもウィンドウ応答性を維持する。
- キーボード: `MakeCode` と E0/E1 フラグ等を正規化して安定した物理キー ID に変換する。**Key Down の初回だけ**1 回。長押しによる自動リピートは除外。左右修飾キー、テンキー、JIS 固有キーを個別検証する。
- フォーカス移動・休止・ロック復帰で Key Up が欠落して押下状態が残る可能性があるため、押下状態の再同期規則を実測で設計する。`Fn` など OS に通常届かないキーは対象外。
- マウス: **相対移動**について、各入力イベントの `sqrt(dx² + dy²)` を合計する。絶対移動イベントをこの式へ直接投入しない。
- マウス counts の積算は整数固定小数点（`millicounts`、1 count = 1000）で記録し、64 ビット整数を使用する。CPI を指定した場合のみ推定物理距離を表示する。
- Raw Input の相対変位はマウスセンサーの生データと必ずしも同一ではない。高 DPI / 複数マウス / タッチパッド / リモートデスクトップで差異がないか検証する。
- スレッド例外・DB 保存失敗・イベントキューのオーバーフローが発生したら、黙って正常値に見せず診断ログまたは状態表示に記録する。

### 4.3 計測セッションの生成

- **アプリの計測プロセスを起動するたびに新しい `recording_id`（UUID）を生成・永続化する。**
- インストールごとに `device_id`（UUID）を作成し、SQLite に永続化する。
- これにより再起動・DB 復元によるカウンター巻き戻りを同一集計と誤認しない。
- ただしクローンしたインストール/DB を複数 PC で同時使用しないこと。バックアップ復元による `device_id` の複製は検知して再登録できるようにする。
- UTC の `hour_start = floor(unix_time / 3600) * 3600` をキーにし、時刻を跨いだら別行へ集計する。
- システム時計の大きな飛びを検知してログに残す（時計誤設定時の所属時間の厳密な修復は MVP 範囲外）。

## 5. クラウド DB 設計（D1）

### 5.1 テーブル

| テーブル | 主キー | 主な列・役割 |
| --- | --- | --- |
| `users` | `id` | `username` unique、`data_generation`、`created_at` |
| `passkeys` | `credential_id` | `user_id`、公開鍵、署名カウンター、表示名、登録/最終使用時刻、失効時刻 |
| `devices` | `id` | `user_id`、表示名、作成/最終接続/失効時刻 |
| `device_sessions` | `id` | `user_id`、`device_id`、refresh token のハッシュ、作成/有効期限/失効時刻 |
| `recovery_codes` | `id` | `user_id`、コードハッシュ、発行/使用時刻 |
| `recordings` | `id` | `device_id`、`data_generation`、`started_at`、`ended_at`、`key_schema_version` |
| `mouse_hourly` | `(recording_id, hour_start)` | `distance_millicounts`（64-bit 非負整数） |
| `key_hourly` | `(recording_id, hour_start, key_code)` | `press_count`（64-bit 非負整数） |

最低限の集計スキーマは次のとおり。`recordings`・外部キー・索引は Drizzle の実装時に作成する。

```sql
CREATE TABLE mouse_hourly (
  recording_id TEXT NOT NULL REFERENCES recordings(id),
  hour_start INTEGER NOT NULL,
  distance_millicounts INTEGER NOT NULL DEFAULT 0
    CHECK (distance_millicounts >= 0),
  PRIMARY KEY (recording_id, hour_start)
);

CREATE TABLE key_hourly (
  recording_id TEXT NOT NULL REFERENCES recordings(id),
  hour_start INTEGER NOT NULL,
  key_code INTEGER NOT NULL,
  press_count INTEGER NOT NULL DEFAULT 0
    CHECK (press_count >= 0),
  PRIMARY KEY (recording_id, hour_start, key_code)
);
```

索引は `recordings(device_id)`、`devices(user_id)`、必要に応じて `mouse_hourly(hour_start)`・`key_hourly(hour_start)` から着手し、実クエリの計画を見て調整する。

### 5.2 ローカル SQLite

- `recordings`、`mouse_hourly`、`key_hourly` の計測列は基本的にクラウドと対応する。
- **ローカル専用:** 各行の `local_revision`、`acked_revision` または最終確認済み値、`sync_state`、`app_settings`、`remote_cache`。
- ローカルDBは Rust で migration を行う。D1 は Void + Drizzle migrations で管理し、スキーマを完全に一致させる必要はない。
- `journal_mode=WAL`、トランザクションによる原子的なバッチ保存を利用し、統計UIと DB 書き込みの競合を避ける。
- アカウント登録前の録画/記録もローカルに残す。クラウドへの初回アップロードは利用者が明示的に選択する。

### 5.3 キー ID と単位

- `key_code` は固定した**物理キー ID**。入力文字ではない。内部の正規化テーブルを `key_schema_version` で管理する。
- JIS/US の表示レイアウトは UI のマッピングで切り替え、保存済みの値を書き換えない。
- 互換性が不明なキーは「Unknown」に集計し、診断用に対応可否を記録する。入力シーケンスや文字列は記録しない。
- マウス: `distance_millicounts` は raw 移動量の近似積算であり cm ではない。CPI の異なる複数マウスの raw counts を足した数値は物理距離として解釈しない。MVP では raw counts として表示し、CPI 対応は検証後に有効化する。
- 将来、クリックやスクロールを追加するときは別の集計列/テーブルを追加し、計測のコアを破壊しない。

## 6. 同期プロトコル

### 6.1 基本仕様

- **ローカル DB 保存:** 5 秒ごと（ほか、正常終了時・必要なタイミング）。
- **クラウド同期:** 原則 5 分ごと。起動時、ネットワーク復帰時、ログイン直後、ユーザーの「今すぐ同期」操作でも実行。
- **送信内容:** 変更された集計行の **そのセッション内累計値**（差分ではない）。小さなバッチに分割。
- **失敗時:** オフラインを許容し、指数バックオフ + jitter でリトライ。書き込みの失敗を成功として扱わない。
- **同期 ON/OFF:** 独立した設定。OFF の間も計測とローカル保存は継続する。

### 6.2 冪等性と競合

クラウドは `(recording_id, hour_start[, key_code])` を一意キーとして `MAX(既存値, 受信値)` で UPSERT する。

```sql
INSERT INTO key_hourly (recording_id, hour_start, key_code, press_count)
VALUES (?, ?, ?, ?)
ON CONFLICT(recording_id, hour_start, key_code)
DO UPDATE SET press_count = MAX(key_hourly.press_count, excluded.press_count);
```

- 同じセッションから同じ累計を何回送っても値は増えない。
- アプリ再起動後は別 `recording_id` なので、同じ時間帯でも正しく合算できる。
- 通信途中でローカル行の revision が進んだ場合、古い送信への ACK で新しい変更を同期済みにしない（ACK は送信時 revision に対して反映）。
- 同期 API は常に認証済みユーザーが当該 device/recording を所有するか確認する。`user_id` をリクエスト本文の申告だけで信用しない。
- `recording_id` を最初に登録した所有デバイスへ固定し、ID の再利用や他ユーザーへの付け替えを禁止する。

### 6.3 複数 PC・表示時の重複排除

- 全 PC の合計は、**セッションごとの集計値を合算**する。
- ローカルで現在の PC のデータを表示する場合、クラウドから取得する他 PC の統計には**現在の `device_id` を含めない**。表示値 = 「現 PC のローカル値」+「その他 PC のクラウド集計」。
- 同じ PC のクラウド上の過去データは、起動時/ログイン時に `recording_id` キーでローカルへ復元し、`MAX` でマージする。これで同じ PC の過去データが抜けない。
- クラウド取得結果はローカルにキャッシュし、オフライン中は最終取得時刻を明示して表示する。別 PC の最新値は同期されるまで反映されない。
- 再インストールで以前の `device_id` を保持しない場合は「新しい PC」として登録される。以前のデバイスのデータは、クラウド上の別デバイス分として集計に残る。
- 同じ物理 PC で複数の Tauri インストールを実行する場合も、異なる `device_id` として扱う（重複計測の可能性を UI 上で注意喚起）。

### 6.4 データ削除と古い PC

- `users.data_generation` をインクリメントする削除操作を用意する。
- 各 `recordings` は作成時の `data_generation` に紐づける。**旧世代の同期はサーバーで拒否**する。
- 過去の統計を削除した後、オフラインだった PC が再接続しても古い記録を自動的に復活させない。
- 再接続 PC は世代変更を検知し、ローカルの旧データをそのまま新世代へ付け替えない。削除/保持の確認を明示する。
- デバイスの失効時はセッショントークンを拒否し、当該 PC のアップロードを停止する。

## 7. Passkey 認証・アカウント・セッション

### 7.1 アカウント

- ユーザー名は一意。ログインは **ユーザー名 + WebAuthn / Passkey**。
- メールアドレスやパスワードは要求しない。
- 登録/認証時にサーバーがチャレンジを生成・短時間保存し、署名・`origin`・`rpId`・用途・有効期限を検証する。
- ブラウザの WebAuthn はHTTPS の認証用ドメインで実施する。Tauri WebView 上の WebAuthn 動作を前提にしない。

### 7.2 新しい PC の登録

1. Tauri からシステムブラウザでサインイン用 HTTPS ページを開く（`state` と PKCE チャレンジを含む）。
2. ブラウザでユーザー名 + Passkey を認証する。
3. サーバーで短命・単回使用の認可コードを発行する。
4. Tauri Deep Link などでコードを受け取り、`state` と PKCE を検証してデバイスのセッションを確立する。
5. ローカル側 `device_id` をクラウドに登録し、明示的な同期設定に従って記録を同期する。

Deep Link はプロトコル横取り等の可能性があるため、**コードは単回・短命・PKCE と紐づけ**、URL にアクセストークンを載せない。使える環境ではループバックリダイレクトも比較検証する。

### 7.3 セッション管理と復旧

- 短命アクセストークン + ローテーションする refresh token。サーバーは refresh token の**ハッシュ**のみを保存し、失効を管理する。
- Windows 側では DPAPI / Windows Credential Manager 等の OS 保護領域にシークレットを置く。SQLite の平文列には保存しない。
- パスキーを複数登録・表示名変更・失効できる。アカウント管理操作には再認証（step-up）を要求する。
- すべての Passkey が使えなくなっても、十分に信頼された**ログイン済み端末**から追加登録できる復旧経路を用意する。ただし、セッションを盗んだ攻撃者が登録できないよう、OS 側の本人確認/端末に紐づく秘密鍵などの追加認証を PoC で検証する。
- さらに任意の緊急復旧手段として、128-bit 以上のランダムな**単回使用リカバリーコード**を発行できる設計とする。ハッシュのみ保管し、使用後に失効させる。
- Passkey・ログイン済み端末・リカバリーコードをすべて失った場合、メールサポートで本人確認する仕組みは MVP にはないため、原則復旧不能と明示する。
- 端末失効の反映、トークンローテーションの競合、Passkey 0 カウンターの扱いを認証テストに含める。

## 8. UI / 統計仕様

### 8.1 画面

1. **ダッシュボード:** 今日/今週/今月/累計のキー数・マウス counts、52 週 × 7 日の細いカレンダーヒートマップ、24 時間の推移、デバイス切替。
2. **キーボード分析:** キー別合計、物理配列ヒートマップ、キーランキング、JIS/US の表示切替。
3. **設定・アカウント:** 自動起動、計測 ON/OFF、同期 ON/OFF、同期日時、デバイス一覧/失効、Passkey 管理、リカバリー、データ削除、プライバシー説明。

### 8.2 共通ルール

- 期間: 今日、過去 7 日、30 日、全期間、任意の開始/終了日。
- フィルター: 全デバイス / 個別デバイス。ローカル専用時は現在の端末のみ。
- タイムスタンプは UTC の時間枠で保存し、表示は選択された IANA タイムゾーン（初期値は OS のローカル TZ）で日別に集計する。DST を考慮する。
- 年間カレンダーは**日別合計**を 1 日 1 セルで表示し、キー入力数 / マウス counts を切り替え。異単位を足した「活動量」は作らない。
- 24 時間ヒートマップはローカル時間帯で表示。日付を跨ぐ表示、DST の重複時刻はラベルとツールチップで区別する。
- キー別ヒートマップは押下回数のみを表示し、キーの入力順は復元できない。
- 0 回と「欠測/計測停止」は区別できる範囲で UI 表示を分ける（MVP は計測ステータス/最終更新時刻を基本表示）。
- 同期状況を `ローカルのみ / 同期中 / 同期済み / オフライン / エラー` と最終成功時刻で表示する。
- マウス物理距離を表示する場合は利用者が CPI を設定した後に「推定」と明記する。MVP では raw counts を基本表示する。

## 9. プライバシー・セキュリティ

- 入力文字、キー押下順序、対象アプリ名、ウィンドウ名、クリップボードを**保存しない**。
- 可能な限り生イベントは Rust メモリ内だけに保持し、SQLite/D1 には時間別集計値だけを保存する。
- キー別頻度も行動推測につながるデータなので、クラウド送信は**明示的オプトイン**、通信は HTTPS、閲覧は本人認証必須とする。
- バックグラウンド計測中であることをトレイで判別可能にし、いつでも停止できるようにする。
- API には認証・所有権検証、リクエスト件数/値上限、レート制限、正規化済みキー ID の検証を設ける。
- ログにキー入力イベント、パスキーの秘密情報、認証トークン、復旧コードを出力しない。
- アカウント全削除と統計削除を分ける。削除の可視的な確認、同期世代による再アップロード防止を設ける。
- 端末トークン流出・デバイス紛失時にはサーバーから失効可能にする。
- ユーザー向けのデータエクスポート（CSV/JSON）を MVP 後半の候補とする。

## 10. HTTP API（案）

| メソッド | パス | 概要 |
| --- | --- | --- |
| `POST` | `/api/v1/auth/register/options` | WebAuthn 登録チャレンジ |
| `POST` | `/api/v1/auth/register/verify` | 公開鍵登録、アカウント作成 |
| `POST` | `/api/v1/auth/login/options` | WebAuthn 認証チャレンジ |
| `POST` | `/api/v1/auth/login/verify` | 署名検証 |
| `POST` | `/api/v1/auth/device/exchange` | PKCE付き認可コードの交換 |
| `POST` | `/api/v1/auth/refresh` | セッション更新/ローテーション |
| `POST` | `/api/v1/auth/logout` | セッション失効 |
| `GET/POST/DELETE` | `/api/v1/passkeys` | Passkey 一覧・追加・失効（詳細は REST ルーティング時に整理） |
| `GET/POST` | `/api/v1/devices` | 端末一覧・端末登録 |
| `POST` | `/api/v1/devices/:id/revoke` | 端末失効 |
| `POST` | `/api/v1/sync/push` | 変更行の冪等アップロード |
| `GET` | `/api/v1/sync/pull` | 当該デバイスのクラウド記録をローカルへ復元 |
| `GET` | `/api/v1/stats` | 期間・タイムゾーン・端末フィルターで集計取得 |
| `POST` | `/api/v1/data/reset` | 統計削除 + `data_generation` 更新 |
| `DELETE` | `/api/v1/account` | アカウント削除 |

`/stats` は内部クエリ/レスポンスでローカルデバイスの除外をサポートし、二重計上しない。API 詳細（エラーコード、ページネーション、リクエスト上限、型）は `packages/contracts` に確定する。

## 11. 実装フェーズ

### Phase 0 — 技術 PoC（最優先）

- [ ] Windows の Raw Input で、ウィンドウが非表示の状態でもキー/マウスを取得する。
- [ ] 長押し、左右修飾キー、JIS 配列、再起動、スリープ/ロック復帰、複数マウスを実測する。
- [x] Tauri のイベントループ/トレイ/Single Instance と独立 HWND を同居させる。
- [x] Vue 3 + Vite+ + Tauri の開発/ビルドが通る。
- [ ] Void + Cloudflare Workers + D1 + Drizzle の起動・migration・デプロイが通る。
- [x] Void の Vue 対応と、WebAuthn のブラウザ認証→Tauri 復帰を実装する（OS の実ブラウザからの復帰確認は受け入れ待ち）。

**完了条件:** サンプル入力を取得し、独立したデスクトップ・クラウドの最小構成をビルド/起動できる。

### Phase 1 — オフライン計測基盤

- [x] Rust で入力受信、物理キーの正規化、マウス counts 積算。
- [x] 起動ごと `recording_id` 作成、UTC 時間区切りで集計。
- [x] SQLite migration、5 秒ごとの flush、クラッシュ後の復旧。
- [x] 計測 ON/OFF、トレイ、終了、Windows 自動起動、二重起動禁止。
- [x] 整合性の自動テスト（長押し・再起動・UTC 境界）。

**完了条件:** UI がなくてもローカルに安定計測できる。

### Phase 2 — 統計 UI

- [x] Vue/Tailwind のダークテーマ、トレイから表示。
- [x] 今日/7 日/30 日/全期間、52 週カレンダー、24 時間表示。
- [x] キー別ヒートマップ、US/JIS 配列、ランキング。
- [x] 未同期/欠測の表示、エラー表示、設定画面。

**完了条件:** ローカル専用モードで日常利用できる。

### Phase 3 — アカウントとクラウド同期

- [x] ユーザー名 + Passkey による登録/ログイン。
- [x] PKCE/単回コードによるブラウザ→Tauri への認可結果引き渡し。
- [x] デバイス登録/セッション更新/失効、Windows 側の安全なトークン保存。
- [x] D1 の `MAX` UPSERT、revision ベースの ACK、同期再送。
- [x] 同一 PC のデータ復元、別 PC の集計合算、オフラインキャッシュ。
- [x] Passkey 追加とリカバリー、データ世代を用いた削除保護。

**完了条件:** 2 台の PC と通信断を含む E2E テストで重複/欠落がない。

### Phase 4 — 配布・運用

- [x] GitHub Actions で `vp check` / `vp test` / `vp build`、Rust `fmt` / `clippy` / `test`、Tauri Windows ビルドを定義（GitHub 上の実行は未確認）。
- [x] コード署名・Windows インストーラー・自動更新の手順を整備。
- [x] Cloudflare の migration / deployment とロールバック手順。
- [x] ログ・障害調査・データバックアップ/復元の手順。
- [ ] 小規模実利用で CPU/メモリ/ディスク/ネットワーク負荷を測定。

**完了条件:** 配布可能な MVP 版を作成する。

## 12. 受け入れテスト

- [ ] ウィンドウを閉じても 30 分以上計測が続く。
- [ ] Windows 起動時に自動起動し、オプションを OFF にすると起動しない。
- [ ] アプリ再起動後の同じ UTC 時間枠に、正確に値が加算される。
- [ ] 物理的な長押しが自動リピートで水増しされない。
- [ ] 異なる 2 台の PC の統計が合算され、1 台の分が二重加算されない。
- [ ] 同じ同期リクエストを複数回送ってもクラウド値が増えない。
- [ ] 同期中の追加入力が ACK により失われない。
- [ ] 長時間のオフライン後、通信復帰時に全記録が同期される。
- [ ] Passkey で別 PC を追加でき、デバイス失効で新規同期が拒否される。
- [ ] リカバリー手段が機能し、全認証手段喪失時の制限が説明される。
- [ ] アカウント統計削除後に旧デバイス/旧世代の再送を拒否する。
- [ ] 日付を跨ぐケースとタイムゾーン変更で日別合計の誤計上がない。
- [ ] ネット未接続でアカウントなしの状態でも起動・計測・統計表示できる。
- [ ] ログ、DB、API に文字列/キー押下順序/アクティブアプリ名が残らない。
- [ ] 1000Hz 等の高頻度マウス入力中でも許容負荷と欠測率を確認できる。

## 13. 実装前に検証して確定する点

1. **Raw Input での物理キー正規化:** JIS/US、特殊キー、押下状態の再同期。PoC の実測で確定。
2. **マウス距離の意味:** Raw Input の counts を主指標とし、CPI の換算精度・複数マウスの扱いは実測してから拡張。
3. **Void と Vue/Tauri/Vite+ の組み合わせ:** 公式のテンプレートと CLI で疎通確認。連携が難しければ Cloud API は Void 単体で分離。
4. **Passkey を失った際の端末ベース復旧:** 既存セッションだけで追加登録を許可すると乗っ取りリスクがある。OS/端末証明を含む再認証方法を PoC で確定。
5. **クラウドから現在の PC の記録を復元する API:** 件数が増えても扱えるようページングとインデックス設計を実測で確定。
6. **Cloudflare 使用量:** 多数キー×高頻度同期時の書き込み数、無料枠、上限時のエラー挙動をデプロイ前に確認。
7. **署名と配布:** コード署名の費用、SmartScreen、自動更新鍵の管理方式をリリース前に決定。

---

**現在の着手:** 実装・自動検証結果は docs/ACCEPTANCE.md に記録。固定HTTPSオリジンとCloudflareログイン後の本番検証、物理入力・2台PCでの受け入れ、UTC非整数時間オフセットの日付境界の設計を継続する。


