HTTP API向けの小さなRust製curl互換CLIです。HTTP/1.1とHTTPSに対応しています。

- ダウンロードの上限を `--max-filesize` で指定できます。`--atomic-output -o FILE` は、転送が成功した時点で保存先を差し替えます。
- Kaniでリダイレクトと送受信のサイズ判定を検証しました。[証明の対象と検証結果](https://github.com/adokoy001/rill/blob/a6ef80eb32cd46d3dc92e602a4ade85cb9bb6f92/verification/REPORT.md)
- `rill-0.3.0.zip` にLinux x86_64の実行ファイル、ソース、依存ライブラリの通知を収録しています。ZIPと `SHA256SUMS` を同じ場所に保存して次のコマンドで試せます。

```sh
sha256sum -c SHA256SUMS
unzip rill-0.3.0.zip
./rill-0.3.0/dist/linux-x86_64/rill --version
```
