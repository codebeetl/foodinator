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
        "meal_plan_entries",
        "meal_attendance",
        "consumer_meal_preferences",
        "app_settings",
        "ha_calendar_sync",
    ] {
        assert!(
            tables.iter().any(|t| t == expected),
            "expected table `{expected}` to exist after migration, found: {tables:?}"
        );
    }
    assert!(
        !tables.iter().any(|t| t == "eating_slots"),
        "eating_slots should have been dropped in favor of one meal per day, found: {tables:?}"
    );

    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn consumers_have_an_is_default_flag_and_meals_are_trigram_searchable(
    pool: sqlx::PgPool,
) -> sqlx::Result<()> {
    let is_default: bool = sqlx::query_scalar(
        "SELECT column_default::boolean FROM information_schema.columns \
         WHERE table_name = 'consumers' AND column_name = 'is_default'",
    )
    .fetch_one(&pool)
    .await?;
    assert!(!is_default, "is_default should default to false");

    sqlx::query("INSERT INTO meals (name) VALUES ('Spaghetti Bolognese')")
        .execute(&pool)
        .await?;
    let matches: Vec<String> = sqlx::query_scalar("SELECT name FROM meals WHERE name % 'spagetti'")
        .fetch_all(&pool)
        .await?;
    assert_eq!(
        matches,
        vec!["Spaghetti Bolognese"],
        "pg_trgm similarity should tolerate a typo"
    );

    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn meal_plan_entries_have_a_guest_names_array_defaulting_empty(
    pool: sqlx::PgPool,
) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO meals (name) VALUES ('Tacos')")
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO meal_plan_entries (entry_date, meal_id) VALUES ('2026-08-08', 1)")
        .execute(&pool)
        .await?;

    let guest_names: Vec<String> = sqlx::query_scalar(
        "SELECT guest_names FROM meal_plan_entries WHERE entry_date = '2026-08-08'",
    )
    .fetch_one(&pool)
    .await?;
    assert!(guest_names.is_empty());

    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn meal_plan_entries_rejects_a_second_meal_on_the_same_date(
    pool: sqlx::PgPool,
) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO meals (name) VALUES ('Spaghetti Bolognese'), ('Tacos')")
        .execute(&pool)
        .await?;

    let insert_entry =
        "INSERT INTO meal_plan_entries (entry_date, meal_id) VALUES ('2026-08-08', $1)";
    sqlx::query(insert_entry).bind(1_i64).execute(&pool).await?;

    let duplicate = sqlx::query(insert_entry).bind(2_i64).execute(&pool).await;

    assert!(
        duplicate.is_err(),
        "inserting a second meal on a date that already has one should violate the UNIQUE constraint"
    );

    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn consumer_meal_preferences_rejects_an_invalid_preference_value(
    pool: sqlx::PgPool,
) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO consumers (name) VALUES ('Alice')")
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO meals (name) VALUES ('Spaghetti Bolognese')")
        .execute(&pool)
        .await?;

    let invalid = sqlx::query(
        "INSERT INTO consumer_meal_preferences (consumer_id, meal_id, preference) \
         VALUES (1, 1, 'meh')",
    )
    .execute(&pool)
    .await;

    assert!(
        invalid.is_err(),
        "preference should be constrained to 'like'/'dislike', got: {invalid:?}"
    );

    Ok(())
}
