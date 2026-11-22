# rill 0.3.0

シェルからHTTP APIを呼ぶためのRust製CLI。1回の起動で1 URLを扱い、本文を標準出力、エラーを標準エラーに送ります。HTTP/1.1とHTTPSに対応しています。

リダイレクトと送受信のバイト数判定をKaniで形式検証しました。証明した性質、適用範囲、実行記録、再現手順は [安全性検証報告](verification/REPORT.md) にまとめています。

## 試す

ソースからは次のコマンドで起動します。Rust 1.95.0、Linux x86_64で検証しています。ビルドにCargoとCコンパイラを使います。

~~~sh
cargo build --release --locked
./target/release/rill --version
~~~

配布ZIPには `dist/linux-x86_64/rill` も収録しています。Linux x86_64、glibc 2.34以降を目安に利用できます。実行時にcurlやlibcurlは使用しません。

既存のAPIに対する利用例です。URLは手元のサービスに合わせてください。

~~~sh
# HTTPエラーでも本文を表示し、終了コード22でCIに失敗を伝える
./target/release/rill --fail-with-body --json '{"name":"test"}' http://localhost:8000/items

# バイナリを保存し、標準出力にはHTTPステータスだけを書く
./target/release/rill -fsSL -m 10 -o export.zip -w '%{http_code}\n' http://localhost:8000/export

# 標準入力の内容を改行ごと送る
printf 'a\nb\n' | ./target/release/rill -X PUT --data-binary @- http://localhost:8000/content

# 開発用CAで検証してHEADリクエストを送る
./target/release/rill -I --cacert team-root.pem https://localhost:8443/health
~~~

## 失敗しても保存先を守る

~~~sh
./target/release/rill -fSL --max-filesize 20M --atomic-output -o export.zip https://example.com/export.zip
~~~

- `--max-filesize 20M` は最終応答の本文を20 MiBまで許可します。Content-Lengthが上限を超えれば本文の出力前に終了63。長さ不明の応答は実際のバイト数で判定します。ちょうど上限は成功で、超過時の直接出力には上限までの本文が残ることがあります。整数と1024倍単位のK/M/G/T/Pに対応し、0で制限を解除します。`-i` のヘッダーや `-w` の出力は別勘定です。
- `--atomic-output` は保存先と同じディレクトリで一時ファイルを生成します。本文を最後まで受信し、要求した `-w` の出力も成功してからファイルを同期し、保存先を一度に差し替えます。途中切断や上限超過では旧ファイルを保持し、新規の保存先名も作りません。`-f` を指定するとHTTPエラーも失敗になります。`--fail-with-body` と併用したHTTPエラー時にも、作業ファイルは破棄します。
- 新規パスと通常ファイルが対象です。シンボリックリンクやFIFO・デバイスは拒否します。Unixでは完成ファイルも所有者専用の権限になります。差し替え前の所有権、ACL、ハードリンク関係は引き継ぎません。同時保存では最後に差し替えを完了したファイルが残ります。
- 書込み先ディレクトリは信頼できる場所を指定してください。強制終了で `.rill-*` が残る場合があります。ファイルの同期は実施しますが、親ディレクトリの同期を含む電源断後の永続性は保証範囲外です。

## 互換の範囲

| 用途 | 対応する指定 | 挙動 |
|---|---|---|
| APIを呼ぶ | URL、`-X`、`-H`、`-I`、`-i` | GETが初期値。ヘッダーは複数指定できる。HEADやヘッダー付き出力では追跡した各応答のヘッダーを表示する |
| 本文を送る | `-d`、`--data-raw`、`--data-binary`、`--json` | 本文を指定するとPOST。通常の `-d @file` は改行とNULを取り除く。`--json` はContent-TypeとAcceptを設定する。JSONの構文は検査しない |
| 受け取る | `-o`、`-w` | 本文を順次書き出す。`-w` は `%{http_code}`、`%{response_code}`、`%%`、改行、復帰、タブ、バックスラッシュに対応する |
| リダイレクト | `-L`、`--max-redirs N` | 初期値は追跡なし。`-L` は既定で最大50回。通常のPOSTは301/302/303でGETに変わり、同一オリジンの307/308では本文を再送する |
| 成否を判定する | `-f`、`--fail-with-body`、`-s`、`-S`、`-m` | 初期状態では404の本文を表示して終了0。`-f` は本文を抑えて22。`-m` は転送全体の秒数で、小数指定もできる |
| TLS | `--cacert FILE` | 既定はビルドに含めたWebPKIのルート証明書。社内CAのPEM束を指定すると、その束のルートで検証する |
| 操作 | `--help`、`--version`、`--` | 短いオプションの連結（`-fsSL`）、値を続けた指定（`-m5`）、対応オプションの長い名前を使用できる |

自動転送はHTTPSからHTTPへの降格を拒否します。スキーム・ホスト・実効ポートのいずれかが変わる場合は、本文なしGETまたはHEADだけを許可します。指定ヘッダーをすべて外し、その後で元のオリジンに戻っても復活させません。別オリジンへ本文を再送する307/308、または別オリジンへGET・HEAD以外の明示メソッドを送る場合は、接続前に終了コード1を返します。

プロキシ環境変数や `.curlrc` を読まずに直接接続します。URLはスキーム付きのASCII URI（日本語は事前にエンコード）を指定してください。URL内のユーザー情報、URL展開、HTTP/2・3、Cookie jar、multipart、圧縮展開、進捗表示は対象外です。未対応のCLIオプションには終了コード2を返します。

応答ヘッダーはパース後に再構成します。ヘッダー名の大小文字、順序、独自の理由句は元のバイト列と異なることがあります。`-H` は値のあるヘッダーの追加に対応し、Host、Content-Length、Transfer-Encoding、Proxy-Authorizationの手動指定は拒否します。`--data-binary @file` と `--json @file` を単独指定した通常ファイルはディスクから順次読みます。標準入力、フォームファイル、複数のデータ引数などのバッファ入力は、整形前の合計8 MiBまで許可します。超過時は終了26。CA束は4 MiBまでで、超過時は終了77です。

## 実測

同梱バイナリは1,679,200バイト（約1.60 MiB）です。HTTPはureqの同期I/O、TLSはrustls、URLの相対参照解決はfluent-uriを使っています。非同期ランタイムを使わず、本文を最大16 KiBずつ出力します。

以下は前版0.1.0の参考計測です。Linux x86_64で、ループバックのHTTP/1.1サーバーから `-fsS -o /dev/null` でダウンロードしたときの**クライアントプロセスの最大常駐メモリ**です。各5回の中央値を `wait4` の `ru_maxrss` で計測しました。

| 受信量 | rill 0.1.0 | Debian配布のcurl 8.14.1 |
|---|---:|---:|
| 1 MiB | 2,028 KiB | 6,528 KiB |
| 128 MiB | 2,016 KiB | 6,528 KiB |

どちらも128倍の本文をおおむね同じメモリで処理できました。この表の対象は平文HTTPの1転送です。ライブラリ、ビルド設定、TLSの有無によって値は変わります。

## テストを再実行する

~~~sh
cargo build --release --locked
python3 tests/compare.py ./target/release/rill
python3 tests/downloads.py ./target/release/rill
~~~

Python 3、curl 8.14.1を基準とする参照CLI、OpenSSL CLIを使用します。テストはループバックにHTTP/HTTPSサーバーを起動し、一時ディレクトリにテスト専用CAとサーバー証明書を生成します。外部APIやAPIキーは不要です。

本家curl 8.14.1との比較52件を含む計104件を確認しました。比較対象は終了コード、本文のバイト列、出力ファイル、ヘッダーの意味的な内容です。指定ケースでは標準エラーの有無も照合します。証明書の信頼とホスト名、受信途中のタイムアウト、途中切断、POSTリダイレクト、別ポートへの認証情報の転送も含みます。

GitHub Actions用のワークフローはpush・pull requestで `verification/verify.sh` を呼び、整形、単体テスト、Clippy、結合テスト、形式検証、誤変更の検出、依存監査を行います。同じスクリプトをローカルでも実行できます。配布バイナリを置く `dist/` はGitの追跡対象から外しています。

curl側の仕様は [curl man page](https://curl.se/docs/manpage.html) と [機能一覧](https://curl.se/docs/features.html) を参照してください。配布ZIP内の依存ライブラリの通知は `dist/linux-x86_64/THIRD-PARTY-NOTICES.txt` と `dist/linux-x86_64/rust-std-notices.html` に収録しています。

## ライセンス

rill本体のソースコードと実行ファイル（v0.3.0を含む）は [MIT License](LICENSE) で提供します。依存ライブラリとRust標準ライブラリには、それぞれのライセンスが適用されます。
