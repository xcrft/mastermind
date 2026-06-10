// Minimal Express router — fixture for scope-creep auditor eval.
import { Router, Request, Response } from "express";

const router = Router();

router.get("/users", (_req: Request, res: Response) => {
  res.json({ users: [] });
});

router.post("/users", (req: Request, res: Response) => {
  res.status(201).json({ id: "new", ...req.body });
});

export default router;
