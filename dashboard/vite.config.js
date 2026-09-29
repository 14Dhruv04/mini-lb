import { defineConfig, loadEnv } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), '')
  const controlPlaneUrl = env.MINILB_API_BASE_URL || 'https://0zt92gohbe.execute-api.ap-south-1.amazonaws.com'
  const adminToken = env.MINILB_ADMIN_TOKEN || ''
  const localAdminUrl = env.MINILB_LOCAL_ADMIN_URL || 'http://127.0.0.1:9090'

  return {
    plugins: [react()],
    server: {
      host: '127.0.0.1',
      port: 5173,
      proxy: {
        '/control': {
          target: controlPlaneUrl,
          changeOrigin: true,
          secure: true,
          rewrite: (path) => path.replace(/^\/control/, ''),
          configure: (proxy) => {
            proxy.on('proxyReq', (proxyReq) => {
              if (adminToken) proxyReq.setHeader('x-admin-token', adminToken)
            })
          }
        },
        '/local': {
          target: localAdminUrl,
          changeOrigin: false,
          rewrite: (path) => path.replace(/^\/local/, '')
        }
      }
    }
  }
})
