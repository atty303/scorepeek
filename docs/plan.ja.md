# scorepeek 実装計画

この文書には、現在実装されていない作業だけを置く。実装済みの機能、契約、検証結果および
過去の候補はここへ残さず、README、領域別reference、code、testまたはGit historyが所有する。

## 保存データの互換処理を解消する

現在のcodeには、旧schemaを読むreader、migration、fixtureが残っている。利用中の保存dataを
失わずにcurrent-onlyな実装へ移すため、次の順序で処理する。

1. development hostのXDG config/state/cache/data、target host、private corpus metadata、
   設定済みremote metadataをread-onlyでinventoryする。
2. raw frame、score内容、credentialを出力せず、存在するschema/versionと、それを読む
   compatibility経路だけを対応付ける。
3. 到達可能な旧dataごとに、current schemaへのmigration対象、変更範囲、backupまたはrollback、
   検証方法を提示し、外部状態を変更する前にoperatorの承認を得る。
4. 承認済みdataだけを移行し、current readerで読み戻せることを確認する。
5. 到達不能になった旧reader、migration、fixture、schema分岐および説明を同じ論理変更で削除する。

権限または到達性が不足して旧dataの不存在を確認できない場合、そのreaderは削除しない。
同一operator control domain内でも、inventoryはmigrationの承認を兼ねない。

## 完了条件

- 利用中と確認された保存dataがcurrent schemaで読める。
- inventory対象で旧dataが存在しないか、承認済みmigrationが完了したcompatibility経路だけを削除する。
- code、test、設定、READMEおよび領域別referenceに旧versionの維持だけを目的とする分岐や記述が残らない。
- 実装済み項目やpoint-in-timeの検証結果をこのplanへ追記しない。
