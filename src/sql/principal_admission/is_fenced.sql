SELECT EXISTS (SELECT 1 FROM runner_owner_stop_fence WHERE owner_key = $1);
