use sqlx::PgPool;

/// A consumer alongside their standing preference for one particular meal, if
/// any has been set. `preference` is `Some("like" | "dislike")` or `None` for
/// no opinion.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsumerPreference {
    pub consumer_id: i64,
    pub consumer_name: String,
    pub preference: Option<String>,
}

pub async fn list_for_meal(pool: &PgPool, meal_id: i64) -> sqlx::Result<Vec<ConsumerPreference>> {
    sqlx::query_as!(
        ConsumerPreference,
        "SELECT c.id AS consumer_id, c.name AS consumer_name, cmp.preference \
         FROM consumers c \
         LEFT JOIN consumer_meal_preferences cmp \
           ON cmp.consumer_id = c.id AND cmp.meal_id = $1 \
         ORDER BY c.id",
        meal_id
    )
    .fetch_all(pool)
    .await
}

/// Same shape as `list_for_meal`, but for every meal at once - one query for
/// the meals list page's preferences column instead of one per row.
#[derive(Debug, Clone, PartialEq)]
pub struct MealConsumerPreference {
    pub meal_id: i64,
    pub consumer_id: i64,
    pub consumer_name: String,
    pub preference: Option<String>,
}

pub async fn list_for_all_meals(pool: &PgPool) -> sqlx::Result<Vec<MealConsumerPreference>> {
    sqlx::query_as!(
        MealConsumerPreference,
        r#"SELECT m.id AS meal_id, c.id AS consumer_id, c.name AS consumer_name,
         cmp.preference AS "preference?"
         FROM meals m
         CROSS JOIN consumers c
         LEFT JOIN consumer_meal_preferences cmp
           ON cmp.consumer_id = c.id AND cmp.meal_id = m.id
         ORDER BY m.id, c.id"#
    )
    .fetch_all(pool)
    .await
}

/// Sets a consumer's standing preference for a meal. `preference` of `None`
/// clears any existing preference (back to "no opinion"), matching how the
/// edit form's "No opinion" option behaves.
pub async fn set(
    pool: &PgPool,
    consumer_id: i64,
    meal_id: i64,
    preference: Option<&str>,
) -> sqlx::Result<()> {
    match preference {
        Some(pref) => {
            sqlx::query!(
                "INSERT INTO consumer_meal_preferences (consumer_id, meal_id, preference) \
                 VALUES ($1, $2, $3) \
                 ON CONFLICT (consumer_id, meal_id) \
                 DO UPDATE SET preference = $3, updated_at = now()",
                consumer_id,
                meal_id,
                pref
            )
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query!(
                "DELETE FROM consumer_meal_preferences WHERE consumer_id = $1 AND meal_id = $2",
                consumer_id,
                meal_id
            )
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{consumers, meals};

    #[sqlx::test(migrations = "./migrations")]
    async fn list_for_meal_returns_every_consumer_with_no_opinion_by_default(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let alice = consumers::insert(&pool, "Alice").await?;
        let meal = meals::insert(&pool, "Tacos").await?;

        let prefs = list_for_meal(&pool, meal.id).await?;

        assert_eq!(
            prefs,
            vec![ConsumerPreference {
                consumer_id: alice.id,
                consumer_name: "Alice".to_string(),
                preference: None,
            }]
        );

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn set_then_clear_a_preference_round_trips(pool: PgPool) -> sqlx::Result<()> {
        let alice = consumers::insert(&pool, "Alice").await?;
        let meal = meals::insert(&pool, "Tacos").await?;

        set(&pool, alice.id, meal.id, Some("dislike")).await?;
        let prefs = list_for_meal(&pool, meal.id).await?;
        assert_eq!(prefs[0].preference.as_deref(), Some("dislike"));

        set(&pool, alice.id, meal.id, Some("like")).await?;
        let prefs = list_for_meal(&pool, meal.id).await?;
        assert_eq!(
            prefs[0].preference.as_deref(),
            Some("like"),
            "setting again should overwrite, not duplicate"
        );

        set(&pool, alice.id, meal.id, None).await?;
        let prefs = list_for_meal(&pool, meal.id).await?;
        assert_eq!(prefs[0].preference, None);

        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn list_for_all_meals_groups_every_consumer_under_each_meal(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        let alice = consumers::insert(&pool, "Alice").await?;
        let bob = consumers::insert(&pool, "Bob").await?;
        let tacos = meals::insert(&pool, "Tacos").await?;
        let chili = meals::insert(&pool, "Chili").await?;
        set(&pool, alice.id, tacos.id, Some("like")).await?;
        set(&pool, bob.id, chili.id, Some("dislike")).await?;

        let rows = list_for_all_meals(&pool).await?;

        assert_eq!(
            rows,
            vec![
                MealConsumerPreference {
                    meal_id: tacos.id,
                    consumer_id: alice.id,
                    consumer_name: "Alice".to_string(),
                    preference: Some("like".to_string()),
                },
                MealConsumerPreference {
                    meal_id: tacos.id,
                    consumer_id: bob.id,
                    consumer_name: "Bob".to_string(),
                    preference: None,
                },
                MealConsumerPreference {
                    meal_id: chili.id,
                    consumer_id: alice.id,
                    consumer_name: "Alice".to_string(),
                    preference: None,
                },
                MealConsumerPreference {
                    meal_id: chili.id,
                    consumer_id: bob.id,
                    consumer_name: "Bob".to_string(),
                    preference: Some("dislike".to_string()),
                },
            ]
        );

        Ok(())
    }
}
