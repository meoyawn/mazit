# SQLite migrations

`src/database/migrations.rs` owns the migration runner. It embeds the SQL in this
directory using [Refinery](https://github.com/rust-db/refinery). Database startup
must succeed here before the application reads or changes library records.

Add each schema change as a new `V<number>__<description>.sql` file. Never edit,
rename, or delete an applied migration, even to reformat it. Refinery records the
version, name, checksum, and application time in `refinery_schema_history` and
rejects changed or missing applied migrations. All pending SQL and its history
entries run in one transaction, so a failed migration rolls back the batch.
These checks validate migration history; they do not detect arbitrary data damage
or out-of-band schema edits after migration.

The original app used `PRAGMA user_version=1` without migration history. We adopt
that database only if every non-internal schema object matches the embedded V1
schema exactly. V1 preserves the original DDL formatting for this comparison.
Only after verification does Refinery record V1 without replaying table creation.
An unrecognized schema fails without being baselined or reset. Successful startup
clears the legacy version marker, so lost history cannot trigger adoption again.

V1 records the original schema and remains immutable. V2 removes its `settings`
table. On upgrade, configuration code first exports the old storage-location
binding into the library's private `storage-binding` file. It is safe to repeat
this export if the process stops before V2 commits. New databases finish startup
without a settings table; configuration belongs in files.

The remaining application tables are sync checkpoints: `sources` tracks source
identity and sync/publication progress; `episodes` tracks playlist membership,
position, metadata, and completed upload receipts. `episodes_presence` is an index
for that work. Upload completion commits with WAL and `synchronous=FULL`. On restart,
interrupted sources are immediately due, while uploaded episodes retain their
receipts and are excluded from the transfer queue.

Run `task test` for history validation, rollback, adoption, and a subprocess test
that exits abruptly after four of eight uploads and resumes the remaining four.
