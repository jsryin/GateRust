import { createRoot } from 'react-dom/client';
import { App } from './App';
import './app.css';

const root = document.getElementById('app');
if (!root) {
  throw new Error('未找到客户端界面挂载节点');
}

createRoot(root).render(<App />);
