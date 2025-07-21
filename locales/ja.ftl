# Tabs
home-tab = 🏠 ホーム
relays-tab = 📡 リレー
profile-tab = 👤 プロフィール

# Login Screen
login-heading = ログインまたは登録
secret-key-label = 秘密鍵 (nsec or hex, 初回設定時):
secret-key-hint = nsecまたは16進数の秘密鍵を入力してください
passphrase-label = パスフレーズ:
passphrase-hint = 安全なパスフレーズ
confirm-passphrase-label = パスフレーズの確認:
confirm-passphrase-hint = パスフレーズを再入力してください
login-button = 🔑 パスフレーズでログイン
register-button = ✨ 新しい鍵を登録

# Home Screen
timeline-heading = タイムライン
fetch-latest-button = 🔄 最新のステータスを取得
new-post-window-title = 新規投稿
set-status-heading = ステータスを設定
status-input-hint = いまどうしてる？
publish-button = 🚀 公開
cancel-button = キャンセル
status-too-long = 長すぎます！
no-timeline-message = タイムラインはありません。最新のステータスを取得するか、他のユーザーをフォローしてください。

# Relays Screen
current-connection-heading = 現在の接続
reconnect-button = 🔗 リレーに再接続
edit-relay-lists-heading = リレーリストを編集
nip65-relay-list-label = NIP-65リレーリスト
add-relay-button = ➕ リレーを追加
read-checkbox = 読み取り
write-checkbox = 書き込み
discover-relays-label = Discoverリレー (1行に1URL)
default-relays-label = デフォルトリレー (フォールバック用, 1行に1URL)
save-nip65-button = 💾 NIP-65リストを保存して公開

# Profile Screen
profile-heading = あなたのプロフィール
public-key-heading = 公開鍵
copy-button = 📋 コピー
nip01-profile-heading = NIP-01 プロフィールメタデータ
name-label = 名前:
picture-url-label = 画像URL:
nip05-label = NIP-05:
lud16-label = LUD-16 (Lightning Address):
about-label = 概要:
other-fields-label = その他のフィールド (現在読み取り専用):
save-profile-button = 💾 プロフィールを保存
raw-json-heading = Raw NIP-01 プロフィールJSON
logout-button = ↩️ ログアウト

# Misc
fetching-profile-status = NIP-01 プロフィールを取得中...
profile-loaded-status = プロフィールが正常に読み込まれました。
login-failed-status = ログインに失敗しました: { $error }
profile-saved-status = プロフィールが正常に保存されました！
profile-save-failed-status = プロフィールの保存に失敗しました: { $error }
login-prompt = 既存ユーザー: パスフレーズを入力してください。
setup-prompt = 初回セットアップ: 秘密鍵を入力し、パスフレーズを設定してください。
invalid-nip49-format = 設定ファイルのNIP-49フォーマットが無効です。
invalid-nip49-payload = 設定ファイルのNIP-49ペイロードが短すぎます。
incorrect-passphrase = パスフレーズが正しくありません。
invalid-secret-key = 入力された秘密鍵は無効です。
nip49-encryption-error = NIP-49 暗号化エラー: { $error }
relays-not-found = 接続できるリレーがありません。
status-too-long-error = エラー: ステータスが長すぎます！最大 { $max } 文字。
status-published = ステータスが公開されました！イベントID: { $event_id }
status-publish-failed = ステータスの公開に失敗しました: { $error }
event-creation-failed = イベントの作成に失敗しました: { $error }
nip65-publish-warning = 警告: 空のNIP-65リストを公開しようとしています。
nip65-published = NIP-65リストが公開されました！イベントID: { $event_id }
nip65-publish-failed = NIP-65リストの公開に失敗しました: { $error }
nip01-published = NIP-01 プロフィールが公開されました！イベントID: { $event_id }
nip01-publish-failed = NIP-01プロフィールの公開に失敗しました: { $error }
profile-save-error = プロフィール保存操作中にエラーが発生しました: { $error }
client-shutdown-failed = ログアウト時のクライアントシャットダウンに失敗しました: { $error }
logged-out = ログアウトしました。
please-login = ログインしてください。
fetching-statuses = 最新のステータスを取得中...
no-followed-users-for-status = ステータスを取得するフォロー中のユーザーがいません。
no-write-relays = フォロー中のユーザーの書き込み可能なリレーが見つかりませんでした。
fetched-statuses = フォロー中のユーザーの書き込みリレーから{ $count }件のステータスを取得しました。
fetching-metadata-for-authors = { $count }人の著者のメタデータを取得しています。
fetched-profiles = { $count }件のプロフィールを取得しました。
no-new-statuses = フォロー中のユーザーの新しいステータスは見つかりませんでした。
timeline-fetch-failed = タイムラインの取得に失敗しました: { $error }
reconnecting-relays = リレーに再接続中...
relay-connection-successful = リレー接続成功！
relay-connection-failed = リレーへの接続に失敗しました: { $error }
publishing-nip65 = NIP-65リストを公開中...
saving-profile = NIP-01 プロフィールを保存中...
publishing-status = NIP-38ステータスを公開中...
registering-key = 新しいキーを登録しています...
login-attempt = ログインしようとしています...
decrypting-key = 秘密鍵を復号しようとしています...
key-decrypted = 秘密鍵の復号に成功しました。公開鍵: { $pubkey }
login-complete = ログイン処理が完了しました！
registered-and-logged-in = 登録してログインしました。公開鍵: { $pubkey }
registration-failed = 新しいキーの登録に失敗しました: { $error }
passwords-do-not-match = パスフレーズが一致しません。
