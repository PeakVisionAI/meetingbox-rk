# 企业知识服务中心

## 结构

Windows Tauri 的 `knowledge_center.rs` 验证管理员密码，持有进程内模式状态并通过本地随机令牌调用 127.0.0.1:8069。令牌存放在目标机服务目录，ACL仅管理员及SYSTEM可读。模型请求不出本机。

知识中心取得桥接服务 `/runtime/knowledge` 的排他租约，确认没有流式录音或收尾任务，随后复用 `Prepare-FinalSummary.ps1` 暂停三项语音任务并检查大模型与内存。桥接释放声纹模型引用并停止语音推理；退出后解除租约，下次录音恢复模型。准备过程中不能退出，以免卸载与新录音竞争。页面心跳维护租约；异常关闭后租约可到期释放。

`knowledge_server.py` 读取现有会议数据库，知识事件、声音备注、DOCX文档写入独立 `knowledge.sqlite`。归档先提取并人工审核；引用编号映射回原转写ID，原文哈希变化时拒绝旧草稿。重新归档增加版本，检索使用每场会议最新版本。

向量使用 BAAI/bge-small-zh-v1.5，官方修订 `7999e1d3359715c523056ef9478215996d62a620`，CPU 512维。每次通过独立进程嵌入，进程退出后释放模型内存。语义相似度结合中文双字词匹配；周期信息随检索结果提供，由回答明确说明。尚未提供跨会议事实冲突的自动裁决。

Pi harness 为 `@mariozechner/pi-agent-core` 0.73.1，使用同版本 `pi-ai`，上游 https://github.com/badlogic/pi-mono，许可证随npm依赖保存。仅提供 knowledgeSearch、readMeeting、readDocument、createDocument；禁止任意命令与任意文件访问。中文回答与文档生成使用本地35B，关闭长思考，带输入预算、生成上限及串行锁。

## 使用

首页设置左侧进入企业知识服务中心，重新输入管理员密码。会议缓存支持录音试听、逐字稿DOCX、纪要查看和生成。最右侧神经网络按钮提取事件：事件详情、别称、发言人观点、结论、有效周期、原文引用，审核后建立向量索引。

右侧峰瞰智能助手检索已归档事件并展示来源。Word文档支持DOCX导入、查看、生成和打开；旧DOC需要先转换为DOCX。声纹记忆器支持选会议、试听发言片段并保存人工身份备注；此版本未启用自动跨会议身份匹配。

## 部署

现场依赖 Windows Python venv（fastapi/httpx/torch/transformers/python-docx）及Node24，目录 `D:\MeetingBox-Intel\knowledge-center`。复制本目录源码，运行npm ci，模型置于embedding-model；启动 `start-knowledge.ps1`，计划任务名 `MeetingBox-Knowledge`，管理员交互登录触发。该目录包含本地令牌、数据库、音频缓存和exports，均不可提交到Git。

现有脚本依赖现场固定路径/网段，不构成通用安装器。录音和原始会议数据库保持原入口；本服务不修改原转写。

## 验证

运行 `python -m unittest discover -s services/knowledge-center -p test_knowledge.py`。单测模型与向量采用可控替身，仅用于数据边界；主程序验收另用真实35B、真实会议原文与真实向量模型，结果单独记录。

每次涉及LLM的纪要、事件提取或助手任务，均重新取得资源租约并运行预检。续租与退出互斥，短暂连接失败会重试，恢复后重新检查模型资源。知识目录的NTFS权限应限制为Administrators与SYSTEM；原始会议库只读。逐字稿、纪要、普通文档分别记录kind，旧会议正文内的发言人前缀统一在读取层解析。
