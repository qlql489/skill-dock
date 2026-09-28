import { defineConfig } from 'vitepress'

// base 必须与 GitHub Pages 实际访问路径一致:默认项目页地址是
// https://<user>.github.io/<repo>/,所以这里写 '/skill-dock/'。
// 如果以后像 aite 一样绑定自定义域名(站点在根路径),改回 '/'。
export default defineConfig({
  lang: 'zh-CN',
  title: 'SkillDock',
  description: '一个应用,统一管理所有 AI Agent 的 Skills',
  base: '/skill-dock/',
  lastUpdated: false,
  themeConfig: {
    nav: [
      { text: '首页', link: '/' },
      { text: '快速开始', link: '/quick-start/' },
      { text: '功能指南', link: '/guide/' },
      { text: '设置', link: '/settings/' },
      {
        text: '下载',
        link: 'https://github.com/your-github-id/skill-dock/releases',
      },
    ],
    sidebar: [
      {
        text: '快速开始',
        items: [
          { text: '概览', link: '/quick-start/' },
          { text: '下载安装', link: '/quick-start/installation' },
          { text: '第一个技能', link: '/quick-start/first-skill' },
        ],
      },
      {
        text: '功能指南',
        items: [
          { text: '概览', link: '/guide/' },
          { text: '技能库与来源', link: '/guide/library' },
          { text: 'Agents 与安装', link: '/guide/agents' },
          { text: '组合', link: '/guide/combos' },
          { text: '本机 skill 管理', link: '/guide/local-skills' },
          { text: '技能市场', link: '/guide/market' },
          { text: '更新与待办', link: '/guide/updates' },
        ],
      },
      {
        text: '设置与原理',
        items: [
          { text: '概览', link: '/settings/' },
          { text: '外观与通用', link: '/settings/appearance' },
          { text: '应用更新', link: '/settings/app-update' },
          { text: '数据目录与安全模型', link: '/settings/data' },
        ],
      },
    ],
    socialLinks: [
      {
        icon: 'github',
        link: 'https://github.com/your-github-id/skill-dock',
      },
    ],
    outline: {
      level: [2, 3],
    },
  },
})
