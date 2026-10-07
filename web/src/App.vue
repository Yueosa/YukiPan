<script setup>
import TopBar from './components/TopBar.vue'
import Toasts from './components/Toasts.vue'
</script>

<template>
  <TopBar />
  <router-view v-slot="{ Component }">
    <transition name="fade" mode="out-in">
      <component :is="Component" />
    </transition>
  </router-view>
  <Toasts />
</template>
