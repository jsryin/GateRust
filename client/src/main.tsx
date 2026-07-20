import { createRoot } from 'react-dom/client';
import { App } from './App';
import './app.css';
import { desktop } from './lib/desktop';

const root = document.getElementById('app');
if (!root) {
  throw new Error('未找到客户端界面挂载节点');
}

createRoot(root).render(<App />);
void desktop.show().catch((error: unknown) => {
  console.error('显示客户端窗口失败', error);
});
