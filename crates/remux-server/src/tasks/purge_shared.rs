use anyhow::Result;

use crate::AppContext;

/// Deletes all media with the given kinds plus their tags, images, and relations.
pub(super) async fn purge_by_kinds(ctx: &AppContext, kinds: &[&str]) -> Result<()> {
    let kinds_sql = kinds
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(", ");

    let mut conn = ctx
        .db
        .acquire()
        .await?;

    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .ok();

    let result: Result<()> = async {
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *conn)
            .await?;

        sqlx::query(&format!(
            "CREATE TEMP TABLE _purge_batch AS SELECT id FROM media WHERE kind IN ({kinds_sql})"
        ))
        .execute(&mut *conn)
        .await?;
        sqlx::query("CREATE INDEX _purge_batch_id ON _purge_batch(id)")
            .execute(&mut *conn)
            .await?;

        // left_media_id has no FK/cascade — must delete those orphans explicitly.
        sqlx::query(
            "DELETE FROM media_relations \
             WHERE left_media_id  IN (SELECT id FROM _purge_batch) \
                OR right_media_id IN (SELECT id FROM _purge_batch)",
        )
        .execute(&mut *conn)
        .await?;

        sqlx::query("DELETE FROM media_tags WHERE media_id IN (SELECT id FROM _purge_batch)")
            .execute(&mut *conn)
            .await?;

        sqlx::query("DELETE FROM media_images WHERE media_id IN (SELECT id FROM _purge_batch)")
            .execute(&mut *conn)
            .await?;

        sqlx::query(&format!("DELETE FROM media WHERE kind IN ({kinds_sql})"))
            .execute(&mut *conn)
            .await?;

        sqlx::query("DROP TABLE _purge_batch")
            .execute(&mut *conn)
            .await?;

        sqlx::query("COMMIT")
            .execute(&mut *conn)
            .await?;

        Ok(())
    }
    .await;

    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *conn)
        .await
        .ok();

    result?;

    ctx.addons
        .purge_indexes(ctx)
        .await?;

    Ok(())
}
