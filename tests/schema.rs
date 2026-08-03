// Requires DATABASE_URL pointing at a Postgres server the connecting role can create
// temporary databases on - sqlx::test provisions and migrates a fresh database per test.

#[sqlx::test(migrations = "./migrations")]
async fn migrations_create_expected_tables(pool: sqlx::PgPool) -> sqlx::Result<()> {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public'",
    )
    .fetch_all(&pool)
    .await?;

    for expected in [
        "consumers",
        "meals",
        "eating_slots",
        "meal_plan_entries",
        "ha_calendar_sync",
    ] {
        assert!(
            tables.iter().any(|t| t == expected),
            "expected table `{expected}` to exist after migration, found: {tables:?}"
        );
    }

    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn eating_slot_rejects_duplicate_weekday_and_time_for_same_consumer(
    pool: sqlx::PgPool,
) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO consumers (name) VALUES ('Alice')")
        .execute(&pool)
        .await?;

    let insert_slot = "INSERT INTO eating_slots (consumer_id, label, weekday, start_local_time) \
                        VALUES (1, 'breakfast', 0, '08:00')";
    sqlx::query(insert_slot).execute(&pool).await?;

    let duplicate = sqlx::query(insert_slot).execute(&pool).await;

    assert!(
        duplicate.is_err(),
        "inserting the same consumer/weekday/start_local_time twice should violate the UNIQUE constraint"
    );

    Ok(())
}
