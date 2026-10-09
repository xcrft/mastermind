// Minimal Express router.
import { Router, Request, Response } from "express";

const router = Router();

router.get("/users", (_req: Request, res: Response) => {
  res.json({ users: [] });
});

router.post("/users", (req: Request, res: Response) => {
  res.status(201).json({ id: "new", ...req.body });
});

router.get("/health", (_req: Request, res: Response) => {
  res.json({ status: "ok" });
});

export default router;
