-- 存储切片: blob 内容寻址存储、引用配额账本、私有区文件指向 (文档第 6 章)。
-- 唯一真源是磁盘, 库里只存指向与账本; 引用计数 = 各空间指向表里指着该 hash 的行数之和
-- (当前只有 private_refs, 图床/访客指向表随后续切片加入后并入计数)。

CREATE TABLE blobs (
    sha256     TEXT PRIMARY KEY CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    size       BIGINT NOT NULL CHECK (size >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 用量按引用计: 同一内容挂多个路径, 账上加多份, 盘上仍是一份 (文档第 6 章)。
CREATE TABLE usage (
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    space      TEXT NOT NULL CHECK (space IN ('private', 'images', 'guest')),
    used_bytes BIGINT NOT NULL DEFAULT 0 CHECK (used_bytes >= 0),
    PRIMARY KEY (user_id, space)
);

-- 私有区: 目录以文件系统为准 (空目录无行), 文件落盘时写一条指向。
CREATE TABLE private_refs (
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    path       TEXT NOT NULL,
    sha256     TEXT NOT NULL REFERENCES blobs(sha256),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, path)
);

-- 引用计数与 TTL 清扫都按 hash 反查指向。
CREATE INDEX private_refs_sha256_idx ON private_refs (sha256);
