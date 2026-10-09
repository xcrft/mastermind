// Auth middleware — fixture for scope-creep auditor eval.
import { Request, Response, NextFunction } from "express";

export function requireAuth(
  req: Request,
  res: Response,
  next: NextFunction
): void {
  const token = req.headers.authorization;
  if (!token) {
    res.status(401).json({ error: "unauthorized" });
    return;
  }
  next();
}
