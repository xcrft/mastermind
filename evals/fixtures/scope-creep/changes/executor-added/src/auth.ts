// Auth middleware.
import { Request, Response, NextFunction } from "express";

const BEARER_PREFIX = "Bearer ";

export function requireAuth(
  req: Request,
  res: Response,
  next: NextFunction
): void {
  const header = req.headers.authorization ?? "";
  const token = header.startsWith(BEARER_PREFIX)
    ? header.slice(BEARER_PREFIX.length)
    : header;
  if (!token) {
    res.status(401).json({ error: "unauthorized" });
    return;
  }
  next();
}

export function extractToken(header: string): string {
  return header.startsWith(BEARER_PREFIX)
    ? header.slice(BEARER_PREFIX.length)
    : header;
}
