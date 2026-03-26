import { Router, Request, Response } from 'express';

const router = Router();

// Example GET endpoint
router.get('/', (req: Request, res: Response) => {
  res.json({ message: 'Example route working' });
});

// Example POST endpoint
router.post('/', (req: Request, res: Response) => {
  const { data } = req.body;
  res.json({ 
    message: 'Data received',
    received: data,
  });
});

export default router;
