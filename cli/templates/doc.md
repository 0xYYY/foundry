{% for contract in contracts %}
# {{ contract.name }}.sol

    {% match contract.title %}
        {%- when Some with (title) %}
{{ title }}
        {%- when None %}
    {%- endmatch -%}

    {% match contract.author %}
        {%- when Some with (author) %}
**Author: {{ author -}}**
        {%- when None %}
    {%- endmatch -%}

    {% match contract.details %}
        {%- when Some with (details) %}
{{ details -}}
        {%- when None %}
    {%- endmatch %}

    {% match contract.notice %}
        {%- when Some with (notice) %}
*{{ notice -}}*
        {%- when None %}
    {%- endmatch %}

{%- if contract.methods.len() > 0 %}
## Methods
        {%- for (name, methods) in contract.methods %}
### {{ name }}
        {%- for method in methods %}
```solidity
{{ method }}
```

            {% match contract.details %}
                {%- when Some with (details) %}
{{ details -}}
                {%- when None %}
            {%- endmatch %}

            {% match contract.notice %}
                {%- when Some with (notice) %}
*{{ notice -}}*
                {%- when None %}
            {%- endmatch %}

{%- if method.params.len() > 0 %}
#### Parameters

| Name | Type | Description |
|---|---|---|
{% for param in method.params -%}
| {{ param.name }} | {{ param.kind }} | {{ param.doc }} |
{% endfor -%}
{%- endif %}

{%- if method.returns.len() > 0 %}
#### Return Values

| Name | Type | Description |
|---|---|---|
{% for param in method.returns -%}
| {{ param.name }} | {{ param.kind }} | {{ param.doc }} |
{% endfor -%}
{%- endif %}

{%- endfor %}

{%- endfor %}
{%- endif %}

{%- if contract.events.len() > 0 %}
### Events
{%- endif %}

{%- if contract.errors.len() > 0 %}
### Errors
{%- endif %}

{% endfor %}
