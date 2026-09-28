---
layout: home

hero:
  name: "SkillDock"
  text: "一个应用，统一管理所有 AI Agent 的 Skills"
  tagline: "本地文件夹、GitHub 仓库、技能市场 → 同一个中央技能库 → 符号链接安装到 38+ 个 Agent，改完即生效。"
  actions:
    - theme: brand
      text: 下载 macOS / Windows
      link: https://github.com/qlql489/skill-dock/releases
    - theme: brand
      text: 快速上手
      link: /quick-start/
    - theme: alt
      text: 功能指南
      link: /guide/
    - theme: alt
      text: 设置与原理
      link: /settings/

features:
  - title: 一个技能库，所有 Agent 共用
    details: 技能只存一份，通过符号链接分发到 Claude Code、Codex 等 Agent 目录。修改即生效，卸载无残留。
  - title: 三个技能市场内置
    details: skills.sh、SkillHub、ClawHub 的搜索与热门榜单直接搬进应用，一键装进中央库。
  - title: 被动更新，先看 diff
    details: 来源更新只比较远端 SHA，绝不自动拉取；更新前给 git 级预览，确认后才落盘。
  - title: 安全的软链所有权
    details: 真实目录永不覆盖，别人的软链只在显式确认后替换；彻底删除先进废纸篓。
---

<script setup>
import { withBase } from 'vitepress'
</script>

<div style="margin: 28px 0 12px;">
  <img
    :src="withBase('/img/main.png')"
    alt="SkillDock 主界面预览"
    style="display: block; width: min(100%, 960px); height: auto; margin: 0 auto; border-radius: 18px; box-shadow: 0 20px 48px rgba(15, 23, 42, 0.14); border: 1px solid rgba(148, 163, 184, 0.18);"
  />
</div>

## 下载安装

- macOS：从 [Releases](https://github.com/qlql489/skill-dock/releases) 下载 `.dmg` 安装包，支持 Apple Silicon 和 Intel
- Windows：从 [Releases](https://github.com/qlql489/skill-dock/releases) 下载 `.msi` 安装包
- 首次安装如果被系统拦截，直接看 [下载安装](/quick-start/installation)

## 推荐阅读路径

1. 新用户先看 [快速开始](/quick-start/)，五分钟装好第一个技能
2. 日常使用重点看 [功能指南](/guide/)：技能库、Agents、组合、市场
3. 想了解安全边界和数据放在哪，看 [数据目录与安全模型](/settings/data)

## 文档范围

当前文档站围绕三块展开：

- 安装与首次可用配置
- 日常技能管理工作流（来源、安装、组合、市场、更新）
- 设置项与背后的安全模型

有疑问可以先查各页结尾的「常见问题」小节。
