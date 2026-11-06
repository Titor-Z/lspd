import { defineConfig } from 'vite'

export default defineConfig({
  base: '/lspd/',
  build: {
    outDir: 'dist',
    assetsDir: 'assets',
    target: 'esnext',
    minify: 'terser',
    rollupOptions: {
      output: {
        manualChunks: undefined,
      },
    },
  },
})
