import { createRoot } from 'react-dom/client';
import { App } from './App';
import './app.css';

const root = document.getElementById('app');
if (!root) {
  throw new Error('未找到客户端界面挂载节点');
}

createRoot(root).render(
  window.gaterust ? (
    <App />
  ) : (
    <main className="fatal-state startup-failure" role="alert">
      <strong>无法加载客户端</strong>
      <p>客户端组件加载失败，请重新启动应用。</p>
    </main>
  )
);
