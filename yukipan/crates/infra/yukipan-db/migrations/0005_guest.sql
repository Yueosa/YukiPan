-- 访客空间切片: 访客指向 (文档第 4、6 章)。
-- 访客没有目录树也没有公开整仓列表, 只有拿着 url 能下; expires_at 到点由
-- 每小时 TTL 清扫删指向, 引用计数 = private_refs + image_refs + guest_refs。
-- charged_to 记录这条指向记在谁的 guest 配额账上 (匿名上传记 owner,
-- 登录分享记分享者), 删除/清扫时据此减账。

CREATE TABLE guest_refs (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    public_name TEXT NOT NULL UNIQUE,
    orig_name   TEXT NOT NULL,
    sha256      TEXT NOT NULL REFERENCES blobs(sha256),
    size        BIGINT NOT NULL CHECK (size >= 0),
    source_ip   INET NOT NULL,
    charged_to  UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at  TIMESTAMPTZ NOT NULL
);

-- TTL 清扫按过期时间扫; 引用计数按 hash 反查。
CREATE INDEX guest_refs_expires_at_idx ON guest_refs (expires_at);
CREATE INDEX guest_refs_sha256_idx ON guest_refs (sha256);
