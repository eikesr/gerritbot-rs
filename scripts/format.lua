-- Get the Gerrit base URL from the given change URL.
function get_gerrit_base_url(change_url)
    return string.sub(change_url, 1, #change_url - string.find(string.reverse(change_url), "/"))
end

-- Get a URL for a Gerrit query.
function get_query_url(base_url, query, ...)
    return string.format("%s/q/%s", base_url, string.format(query, ...))
end

-- Format a link.
function format_link(text, target)
    return string.format("[%s](%s)", text, target)
end

-- Format a link to a Gerrit query.
function format_query_link(base_url, text, query, ...)
    return format_link(text, get_query_url(base_url, query, ...))
end

-- Format a link to a user.
function format_user(base_url, user, role)
    return format_query_link(
        base_url,
        user.name or user.email,
        "%s:%s+status:open",
        role, user.email
    )
end

-- Format a change's subject.
function format_change_subject(change)
    return format_link(change.subject, change.url)
end

-- Format a change's project.
function format_change_project(base_url, change)
    local result = format_query_link(
        base_url,
        change.project,
        "project:%s+status:open",
        change.project
    )

    if change.branch ~= "master" then
        result = result .. ", branch:" .. change.branch
    end

    if change.topic then
        result = result .. ", topic:" .. format_query_link(
            base_url,
            change.topic,
            "topic:%s+status:open",
            change.topic
        )
    end

    return result
end

-- Filter and format messages
-- return nil to filter the message
function format_approval(event, approval, is_human)
    if approval.type ~= "Code-Review" and approval.type ~= "WaitForVerification" and approval.type ~= "Verified" then
        return
    end

    local change = event.change
    local base_url = get_gerrit_base_url(change.url)

    local msg = format_change_subject(change) .. " (" .. format_change_project(base_url, change) .. ")"

    local approval_value = tonumber(approval.value)

    if string.match(approval.type, "WaitForVerification") then
        icon = "⌛"
    elseif approval_value > 0 then
        icon = "👍"
    elseif approval_value == 0 then
        icon = "📝"
    else
        icon = "👎"
    end

    local sign = ""
    if approval_value > 0 then
        sign = "+"
    end

    msg = msg .. string.format(" %s %s%s (%s)", icon, sign, approval_value, approval.type)
    msg = msg .. " from " .. format_user(base_url, event.author, "reviewer")

    local len = 0
    local lines = {}
    for line in string.gmatch(event.comment, "[^\r\n]+") do
        if is_human and not line:match "^Patch Set" and not line:match "%(%d+ comments?%)" then
            table.insert(lines, "> " .. line)
            len = len + 1
        elseif string.match(line, "FAILURE") then
            table.insert(lines, "> " .. line)
            len = len + 1
        end
    end

    if len == 0 then
        return msg
    else
        lines = table.concat(lines, "<br>\n")
        return msg .. "\n\n" .. lines
    end
end
