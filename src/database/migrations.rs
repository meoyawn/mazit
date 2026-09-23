use anyhow::{Context, Result, ensure};
use refinery::{Runner, Target};
use rusqlite::Connection;

mod embedded {
    refinery::embed_migrations!("migrations");
}

fn runner() -> Runner {
    embedded::migrations::runner()
        .set_abort_divergent(true)
        .set_abort_missing(true)
        .set_grouped(true)
}

/// Validate migration history and atomically apply pending schema changes.
pub fn run(connection: &mut Connection) -> Result<()> {
    let has_history: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name IS 'refinery_schema_history')",
        [],
        |row| row.get(0),
    )?;
    if !has_history && !schema(connection)?.is_empty() {
        adopt_initial_schema(connection)?;
    }
    runner()
        .run(connection)
        .context("Migrate library database")?;
    // Retire the legacy marker so lost history cannot silently trigger adoption again.
    connection.execute_batch("PRAGMA user_version=0;")?;
    Ok(())
}

fn schema(connection: &Connection) -> Result<Vec<(String, String, String)>> {
    let mut query = connection.prepare(
        "SELECT type,name,sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY type,name",
    )?;
    Ok(query
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

fn adopt_initial_schema(connection: &mut Connection) -> Result<()> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let initial = Connection::open_in_memory()?;
    initial.execute_batch(include_str!("../../migrations/V1__initial.sql"))?;
    // Only the exact pre-Refinery schema can acquire a baseline without replaying its DDL.
    ensure!(
        version == 1 && schema(connection)? == schema(&initial)?,
        "Untracked database schema differs from Mazit's initial schema; refusing to adopt it"
    );
    runner()
        .set_target(Target::FakeVersion(1))
        .run(connection)
        .context("Record verified initial schema")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use refinery::{Migration, error::Kind};

    fn history(connection: &mut Connection) -> Vec<Migration> {
        runner().get_applied_migrations(connection).unwrap()
    }

    #[test]
    fn creates_schema_and_reopening_preserves_history_and_data() {
        let mut connection = Connection::open_in_memory().unwrap();
        run(&mut connection).unwrap();
        connection
            .execute("INSERT INTO sources(id,kind,youtube_id,url,title,folder) VALUES('sentinel','playlist','test','https://www.youtube.com','keep','playlist-test')", [])
            .unwrap();
        let before = history(&mut connection);
        run(&mut connection).unwrap();
        assert_eq!(history(&mut connection), before);
        assert_eq!(before.len(), 2);
        for applied in &before {
            let migrations = runner();
            let embedded = migrations
                .get_migrations()
                .iter()
                .find(|migration| migration.version() == applied.version())
                .unwrap();
            assert_eq!(applied.checksum(), embedded.checksum());
        }
        let value: String = connection
            .query_row(
                "SELECT title FROM sources WHERE id IS 'sentinel'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(value, "keep");
    }

    #[test]
    fn changed_or_renamed_applied_file_blocks_pending_sql() {
        for (name, sql) in [
            ("V1__initial", "DROP TABLE settings;"),
            (
                "V1__renamed",
                include_str!("../../migrations/V1__initial.sql"),
            ),
        ] {
            let mut connection = Connection::open_in_memory().unwrap();
            run(&mut connection).unwrap();
            let before = schema(&connection).unwrap();
            let migrations = [
                Migration::unapplied(name, sql).unwrap(),
                Migration::unapplied("V2__pending", "DROP TABLE sources;").unwrap(),
            ];
            let error = Runner::new(&migrations)
                .set_abort_divergent(true)
                .set_abort_missing(true)
                .set_grouped(true)
                .run(&mut connection)
                .unwrap_err();
            assert!(matches!(error.kind(), Kind::DivergentVersion(..)));
            assert_eq!(schema(&connection).unwrap(), before);
            assert_eq!(history(&mut connection).len(), 2);
        }
    }

    #[test]
    fn rejects_changed_checksum_and_unknown_applied_version() {
        for sql in [
            "UPDATE refinery_schema_history SET checksum='0'",
            "UPDATE refinery_schema_history SET version=99 WHERE version IS 1",
        ] {
            let mut connection = Connection::open_in_memory().unwrap();
            run(&mut connection).unwrap();
            connection.execute_batch(sql).unwrap();
            let before = schema(&connection).unwrap();
            assert!(run(&mut connection).is_err());
            assert_eq!(schema(&connection).unwrap(), before);
        }
    }

    #[test]
    fn failed_migration_rolls_back_sql_and_history() {
        let mut connection = Connection::open_in_memory().unwrap();
        run(&mut connection).unwrap();
        let before = schema(&connection).unwrap();
        let mut migrations = runner().get_migrations().clone();
        migrations
            .push(Migration::unapplied("V3__valid", "CREATE TABLE pending(id INTEGER);").unwrap());
        migrations
            .push(Migration::unapplied("V4__broken", "INSERT INTO absent VALUES(1);").unwrap());
        assert!(
            Runner::new(&migrations)
                .set_grouped(true)
                .run(&mut connection)
                .is_err()
        );
        assert_eq!(schema(&connection).unwrap(), before);
        assert_eq!(history(&mut connection).len(), 2);
    }

    #[test]
    fn adopts_only_the_exact_initial_schema_without_losing_data() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(include_str!("../../migrations/V1__initial.sql"))
            .unwrap();
        connection
            .execute_batch("PRAGMA user_version=1; INSERT INTO sources(id,kind,youtube_id,url,title,folder) VALUES('sentinel','playlist','test','https://www.youtube.com','keep','playlist-test');")
            .unwrap();
        run(&mut connection).unwrap();
        assert_eq!(history(&mut connection).len(), 2);
        let value: String = connection
            .query_row(
                "SELECT title FROM sources WHERE id IS 'sentinel'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(value, "keep");
    }

    #[test]
    fn refuses_to_baseline_an_unrecognized_schema() {
        for change in [
            "PRAGMA user_version=2;",
            "ALTER TABLE episodes ADD COLUMN unexpected TEXT;",
            "DROP INDEX episodes_presence;",
            "CREATE TRIGGER unexpected AFTER DELETE ON episodes BEGIN DELETE FROM sources; END;",
        ] {
            let mut connection = Connection::open_in_memory().unwrap();
            connection
                .execute_batch(include_str!("../../migrations/V1__initial.sql"))
                .unwrap();
            connection.execute_batch("PRAGMA user_version=1;").unwrap();
            connection.execute_batch(change).unwrap();
            let before = schema(&connection).unwrap();
            assert!(run(&mut connection).is_err());
            assert_eq!(schema(&connection).unwrap(), before);
        }
    }

    #[test]
    fn lost_history_cannot_rebaseline_a_managed_database() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(include_str!("../../migrations/V1__initial.sql"))
            .unwrap();
        connection.execute_batch("PRAGMA user_version=1;").unwrap();
        run(&mut connection).unwrap();
        connection
            .execute_batch("DROP TABLE refinery_schema_history;")
            .unwrap();
        assert!(run(&mut connection).is_err());
    }
}
