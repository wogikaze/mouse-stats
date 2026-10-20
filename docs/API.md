# API v1

認証ページとAPIを同じHTTPSオリジンで提供します。全パスは /api/v1 配下です。GET /api/health は疎通確認用。

JSON本文は100KBまで。使用しない本文もサイズ制限付きで消費します。未認証401、所有権・再認証403、不正入力400、世代・端末ID競合409、サイズ413、レート制限429。エラーは {"error":"CODE"}。ログに本文・トークン・復旧コードは出しません。

| パス                                              | メソッド / 入出力                                                                                                                  |
| ------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| /auth/register/options, /auth/login/options       | POST username, state, pkce(S256), mode(device/manage) → challenge_id, options                                                      |
| /auth/register/verify, /auth/login/verify         | POST challenge_id, response → 単回code + state / 管理トークン                                                                      |
| /auth/device/exchange                             | POST code, state, verifier, device_id, name, recovery_key(P-256 SEC1/base64url) → access_token, refresh_token, user_id, generation |
| /auth/refresh                                     | POST refresh_token, nonce(43文字base64url) → 更新トークン。同じnonceの再送は同じ結果                                               |
| /auth/device/logout                               | POST refresh_token → セッション失効（access期限切れにも対応）                                                                      |
| /auth/logout                                      | POST Bearer → セッション失効                                                                                                       |
| /auth/recover                                     | POST username, code → 5分間の管理トークン                                                                                          |
| /device-recovery/options, /device-recovery/verify | POST 通常Bearer + 端末固有鍵の署名 → 短命ブラウザ認可code                                                                          |
| /auth/device-recovery/exchange                    | POST code → 端末失効を再確認して管理セッション                                                                                     |
| /devices                                          | GET 所有端末一覧                                                                                                                   |
| /devices/:id/revoke                               | POST 管理Bearer → 端末・セッション失効                                                                                             |
| /passkeys                                         | GET 一覧 / POST 公開鍵追加（管理Bearer）                                                                                           |
| /passkeys/options                                 | POST 追加登録オプション（管理Bearer）                                                                                              |
| /passkeys/:id                                     | PATCH 名前変更 / DELETE 失効（最後の有効な鍵は維持）                                                                               |
| /recovery                                         | POST 管理Bearer → 256bit単回コード8個。旧コードを失効                                                                              |
| /sync/push                                        | POST generation, recordings[], rows[]（各最大20） → acked[], generation                                                            |
| /sync/pull                                        | GET 現在の端末の時間別値とrecordingメタデータ                                                                                      |
| /stats                                            | GET 所有端末の時間別値。device / exclude_device / from / to / timezone                                                             |
| /data/reset                                       | POST 管理Bearer → 統計削除・世代更新                                                                                               |
| /account                                          | DELETE 管理Bearer → アカウントと関連データを削除                                                                                   |

packages/contracts が送信形式、物理キーID、整数上限の基準。Rust model.rs と同じwire.jsonで型の一致を検証します。非負のJSON安全整数（最大2^53−1）をSQLiteの64bit整数で保存します。1 count = 1000 millicounts。mouse/key共通のhourlyテーブルでkey_code=-1をマウスに使用し、APIではnullです。入力文字・押下順序・イベント時刻の列はありません。

pull/statsはcursor（初回省略）とnext_cursor（終了時null）で最大1000行ずつ取得します。cursorは不透明なbase64url文字列。recording・UTC時間・キーによるキーセットを使い、メタデータはそのページに含まれるrecordingだけ返します。statsのfrom/toはYYYY-MM-DD、timezoneはIANA形式。空ページでもnext_cursorがあれば続けます。UTC時間枠を指定TZの日付へ変換し、DSTの重複時間はUTC枠とオフセットで区別します。

バッチは20行。認証・所有権検証を含む最悪ケースで50クエリ以下となるテストがあり、[D1の公式上限](https://developers.cloudflare.com/d1/platform/limits/)に合わせています。MAX UPSERTと送信revisionに対するACKで再送と追加入力を保護します。

refresh送信前にnonceをDPAPIへ保存し、応答が失われても同じ結果を再取得できます。サーバーはトークンとnonceのハッシュだけを保存し、再送結果は提示された秘密からHMACで導出。別nonceや次のローテーション後の旧トークン再利用は拒否します。ログアウトの失効要求はDPAPI暗号化した送信待ちファイルへ保存し、オフラインでも後で失効を再試行します。

チャレンジ5分（端末復旧2分）、認可コード60秒・単回。通常のPC接続はPKCE/stateで保護。accessは5分、refreshは30日。管理トークンは5分で期限切れとなりrefreshできません。通常端末トークンだけでは管理操作を許可しません。

端末復旧はWindows Helloでの本人確認とDPAPI保護したP-256鍵を併用します。サーバーは署名・有効端末・有効セッションを確認し、失効端末の復旧を拒否します。鍵の異なるクローンDBは409。アプリから新しいdevice_idで再登録できます。すべてのPasskey・有効端末・復旧コードを失うと復旧できません。
