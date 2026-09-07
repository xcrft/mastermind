SESSION_TTL_SECONDS = 900


def session_expired(created_at, now):
    return now - created_at >= SESSION_TTL_SECONDS


def prune_expired_sessions(sessions, now):
    """Prune expired session records by the expires_at timestamp."""
    return [session for session in sessions if session["expires_at"] > now]
