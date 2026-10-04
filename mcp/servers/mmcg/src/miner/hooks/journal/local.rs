//! Durable local work, retried only under the current capture and reader grants.

use super::*;

impl Journal {
    pub fn enqueue_local(&self, episode: &str) -> Result<(), Error> {
        self.conn.execute(
            "INSERT INTO hook_local_queue(episode) VALUES(?1)
            ON CONFLICT(episode) DO UPDATE SET next_attempt=0",
            [episode],
        )?;
        Ok(())
    }

    pub fn pending_local(&self, grant: &Grant, limit: usize) -> Result<Vec<String>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT q.episode FROM hook_local_queue q
            JOIN hook_episode e ON e.id=q.episode
            WHERE json_extract(e.data,'$.client')=?1
            AND json_extract(e.data,'$.project_root')=?2
            AND json_extract(e.data,'$.generation')=?3
            AND q.next_attempt<=unixepoch() ORDER BY q.next_attempt,q.episode LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![
                grant.client,
                grant.project_root,
                grant.generation,
                limit.min(4) as i64
            ],
            |row| row.get(0),
        )?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn local_completed(
        &self,
        episode: &str,
        revision: &str,
        processor: &Value,
    ) -> Result<bool, Error> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM hook_analysis
            WHERE episode=?1 AND revision=?2 AND processor=?3 AND completed=1)",
            params![episode, revision, hash(processor)],
            |row| row.get(0),
        )?)
    }

    pub fn finish_local(&self, episode: &str) -> Result<(), Error> {
        self.conn
            .execute("DELETE FROM hook_local_queue WHERE episode=?1", [episode])?;
        Ok(())
    }

    pub fn retry_local(&self, episode: &str) -> Result<(), Error> {
        self.conn.execute(
            "UPDATE hook_local_queue SET attempts=min(attempts+1,16),
            next_attempt=unixepoch()+min(60,1 << min(attempts,6)) WHERE episode=?1",
            [episode],
        )?;
        Ok(())
    }

    pub fn local_queue_summary(&self, grant: &Grant) -> Result<Value, Error> {
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master
            WHERE type='table' AND name='hook_local_queue')",
            [],
            |row| row.get(0),
        )?;
        if !exists {
            return Ok(json!({"pending":0,"retried":0}));
        }
        let (pending, retried): (i64, i64) = self.conn.query_row(
            "SELECT count(*),coalesce(sum(q.attempts>0),0)
            FROM hook_local_queue q JOIN hook_episode e ON e.id=q.episode
            WHERE json_extract(e.data,'$.client')=?1
            AND json_extract(e.data,'$.project_root')=?2
            AND json_extract(e.data,'$.generation')=?3",
            params![grant.client, grant.project_root, grant.generation],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(json!({"pending":pending,"retried":retried,
            "trigger":["native_event","native_executor_context","reviewed_task_completion"],
            "idle":"retained_until_next_trigger","model":false}))
    }
}
