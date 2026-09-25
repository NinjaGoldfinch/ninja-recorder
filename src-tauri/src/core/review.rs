//! The VOD review commands (WS9): thin wrappers over `db::review`, which is
//! where the decisions and their tests are. Each one reads the clock here, so
//! the `Db` methods it calls stay testable without one.

use super::Ctx;
use crate::db::review::{
    GameReview, Objective, ObjectiveCategory, ObjectiveStatus, ReviewInput, Takeaway,
    TakeawayOwner,
};

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn open_game_for_recording(ctx: &Ctx, recording_id: i64) -> Result<i64, String> {
    ctx.db.ensure_game_for_recording(recording_id).map_err(|e| e.to_string())
}

pub fn get_game_review(ctx: &Ctx, game_id: i64) -> Result<Option<GameReview>, String> {
    ctx.db.get_game_review(game_id).map_err(|e| e.to_string())
}

pub fn save_game_review(ctx: &Ctx, game_id: i64, review: ReviewInput) -> Result<(), String> {
    ctx.db.upsert_review(game_id, &review).map_err(|e| e.to_string())
}

pub fn set_objective_ticked(
    ctx: &Ctx,
    game_id: i64,
    objective_id: i64,
    ticked: bool,
) -> Result<(), String> {
    ctx.db.set_objective_ticked(game_id, objective_id, ticked).map_err(|e| e.to_string())
}

pub fn list_objectives(
    ctx: &Ctx,
    status: Option<ObjectiveStatus>,
) -> Result<Vec<Objective>, String> {
    ctx.db.list_objectives(status).map_err(|e| e.to_string())
}

pub fn create_objective(
    ctx: &Ctx,
    body: String,
    category: ObjectiveCategory,
) -> Result<Objective, String> {
    ctx.db.create_objective(&body, category, now_millis()).map_err(|e| e.to_string())
}

pub fn update_objective(
    ctx: &Ctx,
    objective_id: i64,
    body: String,
    category: ObjectiveCategory,
) -> Result<Objective, String> {
    ctx.db.update_objective(objective_id, &body, category).map_err(|e| e.to_string())
}

pub fn set_objective_status(
    ctx: &Ctx,
    objective_id: i64,
    status: ObjectiveStatus,
) -> Result<Objective, String> {
    ctx.db
        .set_objective_status(objective_id, status, now_millis())
        .map_err(|e| e.to_string())
}

pub fn add_takeaway(ctx: &Ctx, owner: TakeawayOwner, body: String) -> Result<Takeaway, String> {
    ctx.db.add_takeaway(owner, &body, now_millis()).map_err(|e| e.to_string())
}

pub fn delete_takeaway(ctx: &Ctx, takeaway_id: i64) -> Result<(), String> {
    ctx.db.delete_takeaway(takeaway_id).map_err(|e| e.to_string())
}

pub fn promote_takeaway(
    ctx: &Ctx,
    takeaway_id: i64,
    category: ObjectiveCategory,
) -> Result<Objective, String> {
    ctx.db
        .promote_takeaway(takeaway_id, category, now_millis())
        .map_err(|e| e.to_string())
}

pub fn split_block(ctx: &Ctx, game_id: i64) -> Result<i64, String> {
    ctx.db.split_block(game_id).map_err(|e| e.to_string())
}

pub fn import_review_rows(
    ctx: &Ctx,
    rows: Vec<crate::db::review_import::ImportRow>,
) -> Result<crate::db::review_import::ImportReport, String> {
    ctx.db.import_review_rows(&rows).map_err(|e| e.to_string())
}

pub fn merge_blocks(ctx: &Ctx, into_block_id: i64, from_block_id: i64) -> Result<(), String> {
    ctx.db.merge_blocks(into_block_id, from_block_id).map_err(|e| e.to_string())
}
