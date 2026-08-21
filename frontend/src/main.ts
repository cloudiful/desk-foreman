import { createApp } from 'vue'
import ui from '@nuxt/ui/vue-plugin'
import App from './App.vue'
import router from './router'
import { authState } from './api/auth'
import { setUnauthorizedHandler } from './api/http'
import { i18n } from './i18n'
import './style.css'

setUnauthorizedHandler(() => {
  authState.invalidate()
  if (router.currentRoute.value.path !== '/login') {
    void router.replace('/login')
  }
})

createApp(App).use(router).use(ui).use(i18n).mount('#app')
