<script setup>
// 手写 SVG 图标集 (不引图标库)
const props = defineProps({
  name: { type: String, required: true },
  size: { type: [Number, String], default: 15 },
})

const paths = {
  folder: 'M2 4.5a1 1 0 0 1 1-1h3.6l1.6 1.8H13a1 1 0 0 1 1 1V12a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V4.5Z',
  file: 'M4 1.8h5.2L12 4.6V14a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V2.8a1 1 0 0 1 1-1ZM9 2.2V5h2.8',
  upload: 'M8 11V2M8 2 4.8 5.2M8 2l3.2 3.2M2.5 11v2a1 1 0 0 0 1 1h9a1 1 0 0 0 1-1v-2',
  download: 'M8 2v9m0 0 3.2-3.2M8 11 4.8 7.8M2.5 13.5h11',
  plus: 'M8 3v10M3 8h10',
  eye: 'M1.5 8S3.5 3.5 8 3.5 14.5 8 14.5 8 12.5 12.5 8 12.5 1.5 8 1.5 8Zm6.5 0a2 2 0 1 0-4 0 2 2 0 0 0 4 0Z',
  pencil: 'M11.2 2.3a1.4 1.4 0 0 1 2 2l-8 8L2 13l.7-3.2 8.5-7.5Z',
  move: 'M8 1.5v13M8 1.5 5 4.5M8 1.5l3 3M8 14.5l-3-3m3 3 3-3M1.5 8h13M1.5 8 4.5 5M1.5 8 4.5 11M14.5 8 11.5 5m3 3-3 3',
  share: 'M6 8.5 10 6m-4 1.5L10 10M12.5 4a1.8 1.8 0 1 0-3.5.3 1.8 1.8 0 0 0 3.5-.3Zm0 7.7a1.8 1.8 0 1 0-3.5.3 1.8 1.8 0 0 0 3.5-.3ZM6.3 9.8a1.8 1.8 0 1 0-3.5-.3 1.8 1.8 0 0 0 3.5.3Z',
  trash: 'M2.5 4h11M6 4V2.5a.5.5 0 0 1 .5-.5h3a.5.5 0 0 1 .5.5V4m-7 0 .7 9a1 1 0 0 0 1 .9h5.6a1 1 0 0 0 1-.9l.7-9M6.5 7v4m3-4v4',
  close: 'M3.5 3.5l9 9m0-9-9 9',
  left: 'M9.8 3 5 8l4.8 5',
  right: 'M6.2 3 11 8l-4.8 5',
  link: 'M6.5 9.5a3 3 0 0 0 4.2.2l2-2a3 3 0 0 0-4.2-4.2l-1 1m2 5-2 2a3 3 0 0 1-4.2-4.2l1-1',
  check: 'M2.5 8.5 6 12 13.5 4',
  image: 'M2 3a1 1 0 0 1 1-1h10a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V3Zm3.5 4a1.2 1.2 0 1 0 0-2.4A1.2 1.2 0 0 0 5.5 7ZM2 12l3.5-3.5 2.5 2.5L11 8l3 3',
  copy: 'M5.5 5.5v-3a1 1 0 0 1 1-1h7a1 1 0 0 1 1 1v7a1 1 0 0 1-1 1h-3m-8 3h6a1 1 0 0 0 1-1v-6a1 1 0 0 0-1-1h-6a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1Z',
  tag: 'M2 2h5l7 7-5 5-7-7V2Zm3.2 3.2a.9.9 0 1 0 0-1.8.9.9 0 0 0 0 1.8Z',
  back: 'M6.5 3 2 8l4.5 5M2.5 8H14',
  clock: 'M8 14.5a6.5 6.5 0 1 0 0-13 6.5 6.5 0 0 0 0 13ZM8 4.5V8l2.5 1.5',
  refresh: 'M13.5 8a5.5 5.5 0 1 1-1.6-3.9M13.5 1.5v3h-3',
}
</script>

<template>
  <svg
    :width="size"
    :height="size"
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    stroke-width="1.3"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <path :d="paths[props.name] || ''" />
  </svg>
</template>
